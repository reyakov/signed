use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

use nostr_connect::client::AuthUrlHandler;
use nostr_sdk::prelude::*;

#[derive(Debug)]
pub struct UniversalSignerError(Box<dyn Error + Send + Sync + 'static>);

impl fmt::Display for UniversalSignerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Error for UniversalSignerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&*self.0)
    }
}

impl UniversalSignerError {
    pub fn new<E>(err: E) -> Self
    where
        E: Error + Send + Sync + 'static,
    {
        UniversalSignerError(Box::new(err))
    }
}

/// A type-erased signer whose inner signer can be swapped in-place
/// (e.g. after login/logout). All clones see the swap.
#[derive(Clone, Debug)]
pub struct UniversalSigner {
    inner: Arc<RwLock<Arc<dyn InnerSigner>>>,
}

impl UniversalSigner {
    pub fn new<T>(signer: T) -> Self
    where
        T: AsyncGetPublicKey + AsyncSignEvent + AsyncNip44 + 'static,
        <T as AsyncGetPublicKey>::Error: Error + Send + Sync + 'static,
        <T as AsyncSignEvent>::Error: Error + Send + Sync + 'static,
        <T as AsyncNip44>::Error: Error + Send + Sync + 'static,
    {
        Self {
            inner: Arc::new(RwLock::new(Arc::new(InnerSignerImpl(signer)))),
        }
    }

    /// Swap the inner signer in-place. All clones see the new signer.
    pub fn swap_inner<T>(&self, new_signer: T)
    where
        T: AsyncGetPublicKey + AsyncSignEvent + AsyncNip44 + 'static,
        <T as AsyncGetPublicKey>::Error: Error + Send + Sync + 'static,
        <T as AsyncSignEvent>::Error: Error + Send + Sync + 'static,
        <T as AsyncNip44>::Error: Error + Send + Sync + 'static,
    {
        *self.inner.write().expect("RwLock poisoned") = Arc::new(InnerSignerImpl(new_signer));
    }
}

trait InnerSigner: fmt::Debug + Send + Sync + 'static {
    fn get_public_key_async(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<PublicKey, UniversalSignerError>> + Send + '_>>;
    fn sign_event_async(
        &self,
        unsigned: UnsignedEvent,
    ) -> Pin<Box<dyn Future<Output = Result<Event, UniversalSignerError>> + Send + '_>>;
    fn nip44_encrypt_async<'a>(
        &'a self,
        public_key: &'a PublicKey,
        content: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, UniversalSignerError>> + Send + 'a>>;
    fn nip44_decrypt_async<'a>(
        &'a self,
        public_key: &'a PublicKey,
        payload: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, UniversalSignerError>> + Send + 'a>>;
}

#[derive(Debug)]
struct InnerSignerImpl<T>(T);

impl<T> InnerSigner for InnerSignerImpl<T>
where
    T: AsyncGetPublicKey + AsyncSignEvent + AsyncNip44 + Send + Sync + 'static,
    <T as AsyncGetPublicKey>::Error: Error + Send + Sync + 'static,
    <T as AsyncSignEvent>::Error: Error + Send + Sync + 'static,
    <T as AsyncNip44>::Error: Error + Send + Sync + 'static,
{
    fn get_public_key_async(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<PublicKey, UniversalSignerError>> + Send + '_>> {
        Box::pin(async move {
            AsyncGetPublicKey::get_public_key_async(&self.0)
                .await
                .map_err(UniversalSignerError::new)
        })
    }

    fn sign_event_async(
        &self,
        unsigned: UnsignedEvent,
    ) -> Pin<Box<dyn Future<Output = Result<Event, UniversalSignerError>> + Send + '_>> {
        Box::pin(async move {
            AsyncSignEvent::sign_event_async(&self.0, unsigned)
                .await
                .map_err(UniversalSignerError::new)
        })
    }

    fn nip44_encrypt_async<'a>(
        &'a self,
        public_key: &'a PublicKey,
        content: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, UniversalSignerError>> + Send + 'a>> {
        Box::pin(async move {
            AsyncNip44::nip44_encrypt_async(&self.0, public_key, content)
                .await
                .map_err(UniversalSignerError::new)
        })
    }

    fn nip44_decrypt_async<'a>(
        &'a self,
        public_key: &'a PublicKey,
        payload: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, UniversalSignerError>> + Send + 'a>> {
        Box::pin(async move {
            AsyncNip44::nip44_decrypt_async(&self.0, public_key, payload)
                .await
                .map_err(UniversalSignerError::new)
        })
    }
}

impl AsyncGetPublicKey for UniversalSigner {
    type Error = UniversalSignerError;

    fn get_public_key_async(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<PublicKey, Self::Error>> + Send + '_>> {
        let inner = self.inner.read().expect("RwLock poisoned").clone();
        Box::pin(async move { inner.get_public_key_async().await })
    }
}

impl AsyncSignEvent for UniversalSigner {
    type Error = UniversalSignerError;

    fn sign_event_async(
        &self,
        unsigned: UnsignedEvent,
    ) -> Pin<Box<dyn Future<Output = Result<Event, Self::Error>> + Send + '_>> {
        let inner = self.inner.read().expect("RwLock poisoned").clone();
        Box::pin(async move { inner.sign_event_async(unsigned).await })
    }
}

impl AsyncNip44 for UniversalSigner {
    type Error = UniversalSignerError;

    fn nip44_encrypt_async<'a>(
        &'a self,
        public_key: &'a PublicKey,
        content: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, Self::Error>> + Send + 'a>> {
        let inner = self.inner.read().expect("RwLock poisoned").clone();
        Box::pin(async move { inner.nip44_encrypt_async(public_key, content).await })
    }

    fn nip44_decrypt_async<'a>(
        &'a self,
        public_key: &'a PublicKey,
        payload: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, Self::Error>> + Send + 'a>> {
        let inner = self.inner.read().expect("RwLock poisoned").clone();
        Box::pin(async move { inner.nip44_decrypt_async(public_key, payload).await })
    }
}

/// Opens the NIP-46 auth URL in the default browser.
#[derive(Debug, Clone)]
pub struct SignedAuthUrlHandler;

impl AuthUrlHandler for SignedAuthUrlHandler {
    fn on_auth_url(
        &self,
        auth_url: Url,
    ) -> Pin<Box<dyn Future<Output = Result<(), nostr_connect::error::Error>> + Send + '_>> {
        Box::pin(async move {
            webbrowser::open(auth_url.as_str()).unwrap();
            Ok(())
        })
    }
}
