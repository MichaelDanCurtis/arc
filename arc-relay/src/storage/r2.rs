use aws_sdk_s3::{
    config::{Credentials, Region},
    primitives::ByteStream,
    Client,
};

#[derive(Clone)]
pub struct R2Client {
    client: Client,
    bucket: String,
}

impl R2Client {
    pub async fn from_env(
        account_id: &str,
        access_key: &str,
        secret_key: &str,
        bucket: &str,
    ) -> Self {
        let creds = Credentials::new(access_key, secret_key, None, None, "r2");
        let endpoint = format!("https://{account_id}.r2.cloudflarestorage.com");

        let config = aws_sdk_s3::Config::builder()
            .region(Region::new("auto"))
            .endpoint_url(&endpoint)
            .credentials_provider(creds)
            .force_path_style(true)
            .behavior_version_latest()
            .build();

        let client = Client::from_conf(config);
        Self {
            client,
            bucket: bucket.to_string(),
        }
    }

    pub async fn upload_plugin_archive(
        &self,
        key: &str,
        data: Vec<u8>,
        content_type: &str,
    ) -> Result<String, String> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(ByteStream::from(data))
            .content_type(content_type)
            .send()
            .await
            .map_err(|e| format!("R2 upload failed: {e}"))?;

        Ok(format!(
            "https://{}.r2.cloudflarestorage.com/{}/{}",
            "pub", self.bucket, key
        ))
    }

    pub async fn delete_archive(&self, key: &str) -> Result<(), String> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| format!("R2 delete failed: {e}"))?;
        Ok(())
    }

    pub fn archive_key(plugin_slug: &str, version: &str, platform: &str) -> String {
        format!("plugins/{plugin_slug}/{version}/{platform}.beaconplug")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_key_format() {
        let key = R2Client::archive_key("my-plugin", "1.0.0", "darwin-universal");
        assert_eq!(key, "plugins/my-plugin/1.0.0/darwin-universal.beaconplug");
    }
}
