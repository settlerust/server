use axum::{
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::sync::Barrier;

#[derive(Clone)]
pub struct Object {
    pub bytes: Vec<u8>,
    pub etag: String,
}

#[derive(Default)]
pub struct State {
    pub objects: BTreeMap<String, Object>,
    pub calls: Vec<(Method, String)>,
    pub puts: usize,
    pub gets: usize,
    pub lists: usize,
    pub firehose: Vec<serde_json::Value>,
    pub fail_firehose: bool,
    pub fail_get: Option<usize>,
    pub fail_put: Option<usize>,
    pub fail_put_after_write: bool,
    pub conflict_puts: usize,
    pub barrier: Option<Arc<Barrier>>,
    pub barrier_reads: usize,
}

pub struct Fixture {
    pub state: Arc<Mutex<State>>,
    pub s3: aws_sdk_s3::Client,
    pub firehose: aws_sdk_firehose::Client,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Fixture {
    pub async fn new() -> Self {
        let state = Arc::new(Mutex::new(State::default()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let app = axum::Router::new()
            .fallback(response)
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let s3 = aws_sdk_s3::Client::from_conf(
            aws_sdk_s3::config::Builder::new()
                .behavior_version_latest()
                .endpoint_url(&endpoint)
                .force_path_style(true)
                .retry_config(aws_sdk_s3::config::retry::RetryConfig::disabled())
                .region(aws_sdk_s3::config::Region::new("us-east-1"))
                .credentials_provider(aws_sdk_s3::config::Credentials::new(
                    "test", "test", None, None, "tests",
                ))
                .build(),
        );
        let firehose = aws_sdk_firehose::Client::from_conf(
            aws_sdk_firehose::config::Builder::new()
                .behavior_version_latest()
                .endpoint_url(&endpoint)
                .retry_config(aws_sdk_firehose::config::retry::RetryConfig::disabled())
                .region(aws_sdk_firehose::config::Region::new("us-east-1"))
                .credentials_provider(aws_sdk_firehose::config::Credentials::new(
                    "test", "test", None, None, "tests",
                ))
                .build(),
        );
        Self {
            state,
            s3,
            firehose,
            task,
        }
    }

    pub fn archive(&self) -> Arc<server::storage::aws::S3Archive> {
        Arc::new(server::storage::aws::S3Archive::new(
            self.s3.clone(),
            "test-bucket".into(),
        ))
    }
}

pub fn key(id: &str, first: u64) -> String {
    format!("history/v1/game_id={id}/batch-{first:020}.jsonl.gz")
}

pub fn gzip(records: &[server::model::Record]) -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    for record in records {
        serde_json::to_writer(&mut encoder, record).unwrap();
        encoder.write_all(b"\n").unwrap();
    }
    encoder.finish().unwrap()
}

pub fn error(status: StatusCode, code: &str) -> Response {
    (
        status,
        [("content-type", "application/xml")],
        format!("<Error><Code>{code}</Code><Message>injected</Message></Error>"),
    )
        .into_response()
}

async fn response(
    axum::extract::State(shared): axum::extract::State<Arc<Mutex<State>>>,
    request: axum::extract::Request,
) -> Response {
    let method = request.method().clone();
    let path = request
        .uri()
        .path()
        .strip_prefix("/test-bucket/")
        .unwrap_or("")
        .to_owned();
    let headers = request.headers().clone();
    let query = reqwest::Url::parse(&format!("http://fixture{}", request.uri()))
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect::<BTreeMap<_, _>>();
    let bytes = axum::body::to_bytes(request.into_body(), 12_000_000)
        .await
        .unwrap();
    if method == Method::POST {
        assert!(
            headers["x-amz-target"]
                .to_str()
                .unwrap()
                .ends_with("PutRecordBatch"),
            "history must not call DynamoDB"
        );
        let mut state = shared.lock().unwrap();
        state.firehose.push(serde_json::from_slice(&bytes).unwrap());
        return axum::Json(if state.fail_firehose {
            state.fail_firehose = false;
            serde_json::json!({"FailedPutCount":1,"RequestResponses":[{"ErrorCode":"ServiceUnavailableException","ErrorMessage":"injected"}]})
        } else {
            serde_json::json!({"FailedPutCount":0,"RequestResponses":[{"RecordId":"ok"}]})
        }).into_response();
    }
    let (result, barrier) = {
        let mut state = shared.lock().unwrap();
        state.calls.push((method.clone(), path.clone()));
        if query.contains_key("list-type") {
            state.lists += 1;
            let prefix = query.get("prefix").unwrap();
            let keys = state
                .objects
                .keys()
                .filter(|key| key.starts_with(prefix))
                .map(|key| format!("<Contents><Key>{key}</Key></Contents>"))
                .collect::<String>();
            return ([("content-type", "application/xml")], format!("<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><IsTruncated>false</IsTruncated>{keys}</ListBucketResult>")).into_response();
        }
        if method == Method::PUT {
            state.puts += 1;
            if state.fail_put == Some(state.puts) {
                state.fail_put = None;
                return error(StatusCode::INTERNAL_SERVER_ERROR, "InternalError");
            }
            assert_eq!(headers["content-encoding"], "gzip");
            assert_eq!(headers["content-type"], "application/x-ndjson");
            let matches = match state.objects.get(&path) {
                Some(object) => headers
                    .get("if-match")
                    .is_some_and(|etag| etag == object.etag.as_str()),
                None => headers
                    .get("if-none-match")
                    .is_some_and(|value| value == "*"),
            };
            if !matches {
                return error(StatusCode::PRECONDITION_FAILED, "PreconditionFailed");
            }
            if state.conflict_puts > 0 {
                state.conflict_puts -= 1;
                return error(StatusCode::CONFLICT, "ConditionalRequestConflict");
            }
            let etag = format!("\"version-{}\"", state.puts);
            state.objects.insert(
                path,
                Object {
                    bytes: bytes.to_vec(),
                    etag: etag.clone(),
                },
            );
            if state.fail_put_after_write {
                state.fail_put_after_write = false;
                return error(StatusCode::INTERNAL_SERVER_ERROR, "InternalError");
            }
            return ([("etag", etag)], "").into_response();
        }
        assert_eq!(method, Method::GET);
        state.gets += 1;
        if state.fail_get == Some(state.gets) {
            state.fail_get = None;
            return error(StatusCode::INTERNAL_SERVER_ERROR, "InternalError");
        }
        let barrier = if state.barrier_reads < 2 {
            state.barrier_reads += 1;
            state.barrier.clone()
        } else {
            None
        };
        let result = match state.objects.get(&path) {
            Some(object) => ([("etag", object.etag.clone())], object.bytes.clone()).into_response(),
            None => error(StatusCode::NOT_FOUND, "NoSuchKey"),
        };
        (result, barrier)
    };
    if let Some(barrier) = barrier {
        barrier.wait().await;
    }
    result
}
