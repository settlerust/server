//! DynamoDB holds recovery state and its outbox; S3 holds replay history.
use super::{Archive, Store};
use crate::{
    error::{Error, Result},
    model::{Record, Room},
};
use async_trait::async_trait;
use aws_sdk_dynamodb::{
    Client as Dynamo,
    types::{AttributeValue as A, Delete, Put, TransactWriteItem},
};
use aws_sdk_firehose::{Client as Firehose, types::Record as FirehoseRecord};
use aws_sdk_s3::Client as S3;
use std::collections::HashMap;

mod archive;
mod export;
mod firehose;
mod history;
mod store;
pub use archive::S3Archive;
pub use firehose::FirehoseArchive;
pub use store::DynamoStore;
fn key(id: &str, sort: &str) -> HashMap<String, A> {
    HashMap::from([
        ("game_id".into(), A::S(id.into())),
        ("kind".into(), A::S(sort.into())),
    ])
}
fn decode<T: serde::de::DeserializeOwned>(item: &HashMap<String, A>) -> Result<T> {
    let data = item
        .get("data")
        .and_then(|a| a.as_s().ok())
        .ok_or_else(|| Error::storage("missing DynamoDB data"))?;
    serde_json::from_str(data).map_err(Error::storage)
}
