//! S3/MinIO client wrapper — thin layer over aws-sdk-s3

use aws_sdk_s3::{
    config::{Credentials, Region},
    primitives::ByteStream,
    Client,
};
use tracing::instrument;

/// S3-compatible object store client
#[derive(Clone, Debug)]
pub struct ObjectStore {
    client: Client,
    bucket: String,
}

impl ObjectStore {
    /// Build client from explicit config (endpoint, bucket, creds, region)
    pub async fn new(
        endpoint: &str,
        bucket: &str,
        access_key: &str,
        secret_key: &str,
        region: &str,
    ) -> Self {
        let creds = Credentials::new(access_key, secret_key, None, None, "expresso");
        let config = aws_sdk_s3::Config::builder()
            .endpoint_url(endpoint)
            .region(Region::new(region.to_owned()))
            .credentials_provider(creds)
            .force_path_style(true) // MinIO needs path-style
            .build();
        let client = Client::from_conf(config);
        Self {
            client,
            bucket: bucket.to_owned(),
        }
    }

    /// Upload bytes to key
    #[instrument(skip(self, data), fields(bucket = %self.bucket))]
    pub async fn put(&self, key: &str, data: Vec<u8>, content_type: Option<&str>) -> anyhow::Result<()> {
        let mut req = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(ByteStream::from(data));
        if let Some(ct) = content_type {
            req = req.content_type(ct);
        }
        req.send().await?;
        Ok(())
    }

    /// Download object bytes
    #[instrument(skip(self), fields(bucket = %self.bucket))]
    pub async fn get(&self, key: &str) -> anyhow::Result<Vec<u8>> {
        let resp = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await?;
        let bytes = resp.body.collect().await?.to_vec();
        Ok(bytes)
    }

    /// Delete object
    #[instrument(skip(self), fields(bucket = %self.bucket))]
    pub async fn delete(&self, key: &str) -> anyhow::Result<()> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await?;
        Ok(())
    }

    /// Check if object exists
    #[instrument(skip(self), fields(bucket = %self.bucket))]
    pub async fn exists(&self, key: &str) -> anyhow::Result<bool> {
        match self.client.head_object().bucket(&self.bucket).key(key).send().await {
            Ok(_) => Ok(true),
            Err(e) => {
                let svc = e.into_service_error();
                if svc.is_not_found() {
                    Ok(false)
                } else {
                    Err(svc.into())
                }
            }
        }
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }
}
