use alloy::consensus::SignableTransaction;
use alloy::network::TxSigner;
use alloy::primitives::{Address, ChainId, B256};
use k256::ecdsa::SigningKey;
use thiserror::Error;

use alloy::signers::aws::{AwsSigner, AwsSignerError};
use alloy::signers::wallet::{Wallet, WalletError};
use alloy::signers::{Error, Signature, Signer};

#[derive(Debug)]
pub enum NewUniversalSigner {
    Aws(AwsSigner),
    Local(Wallet<SigningKey>),
}

#[derive(Debug, Error)]
pub enum NewUniversalError {
    #[error("AWS Signer Error: {0}")]
    Aws(AwsSignerError),
    #[error("Local Signer Error: {0}")]
    Local(#[from] WalletError),
}

impl From<AwsSignerError> for NewUniversalError {
    fn from(e: AwsSignerError) -> Self {
        Self::Aws(e)
    }
}

#[async_trait::async_trait]
impl Signer<Signature> for NewUniversalSigner {
    async fn sign_hash(&self, hash: &B256) -> Result<Signature, Error> {
        Ok(match self {
            Self::Aws(signer) => signer.sign_hash(hash).await?,
            Self::Local(signer) => signer.sign_hash(hash).await?,
        })
    }

    async fn sign_message(&self, message: &[u8]) -> Result<Signature, Error> {
        Ok(match self {
            Self::Aws(signer) => signer.sign_message(message).await?,
            Self::Local(signer) => signer.sign_message(message).await?,
        })
    }

    fn address(&self) -> Address {
        match self {
            Self::Aws(signer) => <AwsSigner as Signer>::address(signer),
            Self::Local(signer) => {
                <Wallet<SigningKey> as Signer>::address(signer)
            }
        }
    }

    /// Returns the signer's chain ID.
    fn chain_id(&self) -> Option<ChainId> {
        match self {
            Self::Aws(signer) => signer.chain_id(),
            Self::Local(signer) => signer.chain_id(),
        }
    }

    /// Sets the signer's chain ID.
    fn set_chain_id(&mut self, chain_id: Option<ChainId>) {
        match self {
            Self::Aws(signer) => signer.set_chain_id(chain_id),
            Self::Local(signer) => signer.set_chain_id(chain_id),
        }
    }

    fn with_chain_id(mut self, chain_id: Option<ChainId>) -> Self
    where
        Self: Sized,
    {
        self.set_chain_id(chain_id);
        self
    }
}

#[async_trait::async_trait]
impl TxSigner<Signature> for NewUniversalSigner {
    async fn sign_transaction(
        &self,
        tx: &mut dyn SignableTransaction<Signature>,
    ) -> alloy::signers::Result<Signature> {
        Ok(match self {
            Self::Aws(signer) => signer.sign_transaction(tx).await?,
            Self::Local(signer) => signer.sign_transaction(tx).await?,
        })
    }

    fn address(&self) -> Address {
        match self {
            Self::Aws(signer) => <AwsSigner as Signer>::address(signer),
            Self::Local(signer) => {
                <Wallet<SigningKey> as Signer>::address(signer)
            }
        }
    }
}
