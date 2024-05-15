pub mod kms_keys;
pub mod local_keys;
pub mod universal_signer;

pub use kms_keys::KmsKeys;
pub use local_keys::LocalKeys;
pub use universal_signer::UniversalSigner;

#[async_trait::async_trait]
pub trait KeysSource: Send + Sync + 'static {
    /// Returns a key id and signer
    async fn new_signer(
        &self,
        meta_name: &str,
    ) -> eyre::Result<(String, UniversalSigner)>;

    /// Loads the key using the provided id
    async fn load_signer(&self, id: String) -> eyre::Result<UniversalSigner>;
}
