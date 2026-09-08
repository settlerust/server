terraform {
  required_version = ">= 1.5"
  required_providers {
    aws = { source = "hashicorp/aws", version = "~> 6.0" }
  }
}
provider "aws" { region = var.region }
variable "region" {
  type    = string
  default = "us-west-2"
}
variable "project" {
  type    = string
  default = "settlers-research"
  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{2,30}$", var.project))
    error_message = "Use 3–31 lowercase letters, digits, or hyphens, starting with a letter."
  }
}
variable "server_role_name" {
  type        = string
  default     = null
  description = "Optional existing IAM role to attach the server runtime policy to."
}
data "aws_caller_identity" "current" {}
locals { bucket_name = "${var.project}-${data.aws_caller_identity.current.account_id}-${var.region}" }
