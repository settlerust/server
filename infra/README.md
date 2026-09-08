# AWS deployment resources

This directory declares the current-state/outbox DynamoDB table, encrypted private S3 archive bucket, Firehose delivery stream partitioned by game ID, Glue table with partition projection, Athena workgroup/named query, and least-privilege server runtime policy. It does not deploy a compute host or implement SSO providers.

```sh
terraform init
terraform plan -var='region=us-west-2'
terraform apply -var='region=us-west-2'
terraform output -json environment
```

Review the plan and select the intended account/region before applying. `server_role_name` optionally attaches the generated policy to an existing compute role; otherwise `server_policy_arn` is available for attachment. Configure your research role separately for Glue/Athena access and query-result S3 access.

Build/run the server with `--features aws`, set the environment values from Terraform, set a stable `SETTLERS_JWT_SECRET`, and choose the intended `SETTLERS_MODE`. Local mode can exercise AWS persistence with guest JWTs. Production mode disables guest issuance; register a verified identity provider before offering sign-in to users. Configure HTTPS/WSS on the compute ingress.

| Variable | Purpose |
|---|---|
| `AWS_REGION` | AWS SDK region |
| `SETTLERS_STORAGE=aws` | select cloud adapters |
| `SETTLERS_DYNAMODB_TABLE` | current snapshot / temporary outbox table |
| `SETTLERS_FIREHOSE_STREAM` | event delivery stream |
| `SETTLERS_ARCHIVE_BUCKET` | confirmed-history read-back and REST pagination |
| `SETTLERS_JWT_SECRET` | persistent signing material, at least 32 bytes |
| `SETTLERS_BIND` | listener address, default `127.0.0.1:8080` |
| `SETTLERS_ALLOWED_ORIGINS` | permitted browser REST origins |

The AWS SDK uses its normal credential-provider chain (environment, profiles, or attached workload role). No AWS credentials are sent to players. DynamoDB has point-in-time recovery and server-side encryption; S3 has versioning and public access blocked. The Athena workgroup enforces encrypted results and a 1 GiB scan limit per query.

Firehose buffers per partition; its 60-second buffer setting is not an instantaneous delivery guarantee. The application keeps up to 256 unconfirmed records per game and pauses new commits at that limit. For very fast hosted simulations, use the pure engine or tune the archive delivery/horizon architecture after load tests. Firehose active-partition quotas also constrain the number of simultaneously producing games.

One server owns a game set. Conditional DynamoDB writes protect state, but game leases and cross-node live fan-out are not implemented. Use independently assigned tables/game sets until adding explicit ownership coordination.

## Local adapter integration test

```sh
docker compose -f infra/compose.yaml up -d
SETTLERS_TEST_DYNAMODB_URL=http://127.0.0.1:18000   cargo test -p server --features aws --test aws -- --include-ignored
docker compose -f infra/compose.yaml down
```

The DynamoDB test creates and deletes an isolated table. Firehose/S3 SDK tests use local HTTP fixtures to verify partial failures, gzip slices, and structured payloads. Terraform validation checks provider configuration; it does not demonstrate successful live-account provisioning or live Firehose/Glue/Athena delivery.

Primary references: [DynamoDB transactions](https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/transaction-apis.html), [Firehose partition extraction](https://docs.aws.amazon.com/firehose/latest/dev/dynamic-partitioning-partitioning-keys.html), and [Athena partition projection](https://docs.aws.amazon.com/athena/latest/ug/partition-projection-supported-types.html).
