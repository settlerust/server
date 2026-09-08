resource "aws_glue_catalog_database" "research" { name = replace(var.project, "-", "_") }
resource "aws_glue_catalog_table" "events" {
  name          = "events"
  database_name = aws_glue_catalog_database.research.name
  table_type    = "EXTERNAL_TABLE"
  parameters = {
    "classification"            = "json"
    "projection.enabled"        = "true"
    "projection.game_id.type"   = "injected"
    "storage.location.template" = "s3://${aws_s3_bucket.archive.id}/events/game_id=$${game_id}/"
  }
  partition_keys {
    name = "game_id"
    type = "string"
  }
  storage_descriptor {
    location      = "s3://${aws_s3_bucket.archive.id}/events/"
    input_format  = "org.apache.hadoop.mapred.TextInputFormat"
    output_format = "org.apache.hadoop.hive.ql.io.HiveIgnoreKeyTextOutputFormat"
    ser_de_info { serialization_library = "org.openx.data.jsonserde.JsonSerDe" }
    dynamic "columns" {
      for_each = { schema = "int", engine_version = "string", sequence = "bigint", timestamp_ms = "bigint", state_hash = "string", input_json = "string", public_json = "string" }
      content {
        name = columns.key
        type = columns.value
      }
    }
  }
}
resource "aws_athena_workgroup" "research" {
  name = var.project
  configuration {
    enforce_workgroup_configuration    = true
    publish_cloudwatch_metrics_enabled = true
    bytes_scanned_cutoff_per_query     = 1073741824
    result_configuration {
      output_location = "s3://${aws_s3_bucket.archive.id}/query-results/"
      encryption_configuration { encryption_option = "SSE_S3" }
    }
  }
}
resource "aws_athena_named_query" "timeline" {
  name      = "${var.project}-timeline"
  database  = aws_glue_catalog_database.research.name
  workgroup = aws_athena_workgroup.research.name
  query     = <<-SQL
    SELECT DISTINCT sequence, timestamp_ms,
      json_extract_scalar(input_json, '$.request.command.type') AS command_type,
      json_extract_scalar(input_json, '$.request.command.action.type') AS action_type,
      public_json, state_hash
    FROM events
    WHERE game_id = 'REPLACE_WITH_GAME_UUID'
    ORDER BY sequence
  SQL
}
