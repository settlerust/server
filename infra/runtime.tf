resource "aws_iam_policy" "server" {
  name = "${var.project}-server"
  policy = jsonencode({ Version = "2012-10-17", Statement = [
    { Effect = "Allow", Action = ["dynamodb:GetItem", "dynamodb:Query", "dynamodb:Scan", "dynamodb:PutItem", "dynamodb:DeleteItem", "dynamodb:UpdateItem", "dynamodb:ConditionCheckItem"], Resource = aws_dynamodb_table.state.arn },
    { Effect = "Allow", Action = ["firehose:PutRecordBatch"], Resource = aws_kinesis_firehose_delivery_stream.events.arn },
    { Effect = "Allow", Action = ["s3:ListBucket"], Resource = aws_s3_bucket.archive.arn, Condition = { StringLike = { "s3:prefix" = ["events/*"] } } },
    { Effect = "Allow", Action = ["s3:GetObject"], Resource = "${aws_s3_bucket.archive.arn}/events/*" }
  ] })
}
resource "aws_iam_role_policy_attachment" "server" {
  count      = var.server_role_name == null ? 0 : 1
  role       = var.server_role_name
  policy_arn = aws_iam_policy.server.arn
}
output "server_policy_arn" { value = aws_iam_policy.server.arn }
output "environment" {
  value = {
    AWS_REGION               = var.region
    SETTLERS_STORAGE         = "aws"
    SETTLERS_DYNAMODB_TABLE  = aws_dynamodb_table.state.name
    SETTLERS_FIREHOSE_STREAM = aws_kinesis_firehose_delivery_stream.events.name
    SETTLERS_ARCHIVE_BUCKET  = aws_s3_bucket.archive.id
  }
}
output "athena_workgroup" { value = aws_athena_workgroup.research.name }
output "glue_database" { value = aws_glue_catalog_database.research.name }
