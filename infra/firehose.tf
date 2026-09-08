resource "aws_iam_role" "firehose" {
  name = "${var.project}-firehose"
  assume_role_policy = jsonencode({ Version = "2012-10-17", Statement = [{
    Effect    = "Allow", Principal = { Service = "firehose.amazonaws.com" }, Action = "sts:AssumeRole",
    Condition = { StringEquals = { "aws:SourceAccount" = data.aws_caller_identity.current.account_id } }
  }] })
}
resource "aws_iam_role_policy" "firehose" {
  role = aws_iam_role.firehose.id
  policy = jsonencode({ Version = "2012-10-17", Statement = [{
    Effect   = "Allow", Action = ["s3:AbortMultipartUpload", "s3:GetBucketLocation", "s3:GetObject", "s3:ListBucket", "s3:ListBucketMultipartUploads", "s3:PutObject"],
    Resource = [aws_s3_bucket.archive.arn, "${aws_s3_bucket.archive.arn}/*"]
  }] })
}
resource "aws_kinesis_firehose_delivery_stream" "events" {
  name        = "${var.project}-events"
  destination = "extended_s3"
  depends_on  = [aws_iam_role_policy.firehose]
  extended_s3_configuration {
    role_arn            = aws_iam_role.firehose.arn
    bucket_arn          = aws_s3_bucket.archive.arn
    compression_format  = "GZIP"
    buffering_size      = 64
    buffering_interval  = 60
    prefix              = "events/game_id=!{partitionKeyFromQuery:game_id}/"
    error_output_prefix = "errors/!{firehose:error-output-type}/"
    dynamic_partitioning_configuration { enabled = true }
    processing_configuration {
      enabled = true
      processors {
        type = "MetadataExtraction"
        parameters {
          parameter_name  = "MetadataExtractionQuery"
          parameter_value = "{game_id: .game_id}"
        }
        parameters {
          parameter_name  = "JsonParsingEngine"
          parameter_value = "JQ-1.6"
        }
      }
    }
  }
}
