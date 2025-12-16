#[cfg(feature = "testing")]
pub mod mock;
pub mod real;

use crate::Config;
use anyhow::Result;
use docs_rs_headers::SurrogateKey;
use docs_rs_opentelemetry::AnyMeterProvider;
use docs_rs_types::KrateName;
use std::iter;

pub trait CdnBehaviour {
    fn purge_surrogate_keys<I>(&self, keys: I) -> impl Future<Output = Result<()>> + Send
    where
        I: IntoIterator<Item = SurrogateKey> + 'static + Send,
        I::IntoIter: Send;

    fn queue_crate_invalidation(
        &self,
        krate_name: &KrateName,
    ) -> impl Future<Output = Result<()>> + Send {
        self.purge_surrogate_keys(iter::once(SurrogateKey::from(krate_name.clone())))
    }
}

#[derive(Debug)]
pub enum Cdn {
    Real(real::RealCdn),
    #[cfg(feature = "testing")]
    Mock(mock::MockCdn),
}

/// normal functionality
impl Cdn {
    pub fn from_config(config: &Config, meter_provider: &AnyMeterProvider) -> Result<Self> {
        Ok(Self::Real(real::RealCdn::from_config(
            config,
            meter_provider,
        )?))
    }
}

/// testing functionality
#[cfg(feature = "testing")]
impl Cdn {
    pub fn mock() -> Self {
        Self::Mock(mock::MockCdn::default())
    }

    pub async fn purged_keys(&self) -> Result<docs_rs_headers::SurrogateKeys> {
        let Self::Mock(cdn) = self else {
            anyhow::bail!("found real cdn, no collected purges");
        };

        let purges = cdn.purged.lock().await;
        Ok(purges.clone())
    }
}

impl CdnBehaviour for Cdn {
    async fn purge_surrogate_keys<I>(&self, keys: I) -> Result<()>
    where
        I: IntoIterator<Item = SurrogateKey> + 'static + Send,
        I::IntoIter: Send,
    {
        match self {
            Self::Real(real) => real.purge_surrogate_keys(keys).await,
            #[cfg(feature = "testing")]
            Self::Mock(mock) => mock.purge_surrogate_keys(keys).await,
        }
    }
}
