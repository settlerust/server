# AWS deployment resources

This directory declares the current-state/outbox DynamoDB table, encrypted private S3 archive bucket, compressed S3 history batches, optional Firehose analytics stream partitioned by game ID, Glue table with partition projection, Athena workgroup/named query, and least-privilege server runtime policy. It does not deploy a compute host or implement SSO providers.

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
| `SETTLERS_FIREHOSE_STREAM` | optional raw analytics delivery; omit for S3 history only |
| `SETTLERS_ARCHIVE_BUCKET` | confirmed-history read-back and REST pagination |
| `SETTLERS_JWT_SECRET` | persistent signing material, at least 32 bytes |
| `SETTLERS_BIND` | listener address, default `127.0.0.1:8080` |
| `SETTLERS_ALLOWED_ORIGINS` | permitted browser REST origins |

The AWS SDK uses its normal credential-provider chain (environment, profiles, or attached workload role). No AWS credentials are sent to players. DynamoDB has point-in-time recovery and server-side encryption; S3 has versioning and public access blocked. The Athena workgroup enforces encrypted results and a 1 GiB scan limit per query.

The application writes gzip history directly to `history/v1/game_id=UUID/`, in fixed ranges of up to 100 records. It verifies each pending range before pruning the outbox. Runtime IAM grants history GET/PUT separately from raw analytics reads. The lifecycle rule expires superseded history object versions after one day and keeps current objects. It creates no replacement bucket or table.

The outbox holds up to 256 unconfirmed records per game and pauses new commits at that limit. Optional Firehose delivery uses its own buffering and active-partition quotas; S3 history confirmation never relies on Firehose buffer completion. Raw analytics consumers still deduplicate retries.

Current snapshots, temporary outbox records, and ownership manifests stay in DynamoDB. Ownership checks fence commits when a game is reassigned; an external orchestrator coordinates routing and assignment. History storage is independent of the serving process.

## Local adapter integration test

```sh
docker compose -f infra/compose.yaml up -d
SETTLERS_TEST_DYNAMODB_URL=http://127.0.0.1:18000   cargo test -p server --features aws --test aws -- --include-ignored
docker compose -f infra/compose.yaml down
```

The DynamoDB test creates and deletes an isolated table. Firehose/S3 SDK tests use local HTTP fixtures to verify conditional batch updates, concurrent writers, lost acknowledgments, bounded reads, corrupt gzip batches, and analytics retries. Terraform validation checks provider configuration; it does not demonstrate successful live-account provisioning or live Firehose/Glue/Athena delivery.

Primary references: [DynamoDB transactions](https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/transaction-apis.html), [Firehose partition extraction](https://docs.aws.amazon.com/firehose/latest/dev/dynamic-partitioning-partitioning-keys.html), and [Athena partition projection](https://docs.aws.amazon.com/athena/latest/ug/partition-projection-supported-types.html).
