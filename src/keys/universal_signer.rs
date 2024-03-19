use ethers::core::k256::ecdsa::SigningKey;
use ethers::core::types::transaction::eip2718::TypedTransaction;
use ethers::core::types::transaction::eip712::Eip712;
use ethers::core::types::{Address, Signature as EthSig};
use ethers::signers::{Signer, Wallet, WalletError};
use ethers::types::Bytes;
use thiserror::Error;

use crate::aws::ethers_signer::AwsSigner;

#[derive(Debug)]
pub enum UniversalSigner {
    Aws(AwsSigner),
    Local(Wallet<SigningKey>),
}

impl UniversalSigner {
    pub async fn raw_signed_tx(
        &self,
        tx: &TypedTransaction,
    ) -> eyre::Result<Bytes> {
        let signature = match self {
            Self::Aws(signer) => signer.sign_transaction(tx).await?,
            Self::Local(signer) => signer.sign_transaction(tx).await?,
        };

        Ok(tx.rlp_signed(&signature))
    }
}

#[derive(Debug, Error)]
pub enum UniversalError {
    #[error("AWS Signer Error: {0}")]
    Aws(<AwsSigner as Signer>::Error),
    #[error("Local Signer Error: {0}")]
    Local(#[from] WalletError),
}

impl From<<AwsSigner as Signer>::Error> for UniversalError {
    fn from(e: <AwsSigner as Signer>::Error) -> Self {
        Self::Aws(e)
    }
}

#[async_trait::async_trait]
impl Signer for UniversalSigner {
    type Error = UniversalError;

    async fn sign_message<S: Send + Sync + AsRef<[u8]>>(
        &self,
        message: S,
    ) -> Result<EthSig, Self::Error> {
        Ok(match self {
            Self::Aws(signer) => signer.sign_message(message).await?,
            Self::Local(signer) => signer.sign_message(message).await?,
        })
    }

    async fn sign_transaction(
        &self,
        tx: &TypedTransaction,
    ) -> Result<EthSig, Self::Error> {
        Ok(match self {
            Self::Aws(signer) => signer.sign_transaction(tx).await?,
            Self::Local(signer) => signer.sign_transaction(tx).await?,
        })
    }

    async fn sign_typed_data<T: Eip712 + Send + Sync>(
        &self,
        payload: &T,
    ) -> Result<EthSig, Self::Error> {
        Ok(match self {
            Self::Aws(signer) => signer.sign_typed_data(payload).await?,
            Self::Local(signer) => signer.sign_typed_data(payload).await?,
        })
    }

    fn address(&self) -> Address {
        match self {
            Self::Aws(signer) => signer.address(),
            Self::Local(signer) => signer.address(),
        }
    }

    /// Returns the signer's chain id
    fn chain_id(&self) -> u64 {
        match self {
            Self::Aws(signer) => signer.chain_id(),
            Self::Local(signer) => signer.chain_id(),
        }
    }

    /// Sets the signer's chain id
    fn with_chain_id<T: Into<u64>>(self, chain_id: T) -> Self {
        match self {
            Self::Aws(signer) => Self::Aws(signer.with_chain_id(chain_id)),
            Self::Local(signer) => Self::Local(signer.with_chain_id(chain_id)),
        }
    }
}
