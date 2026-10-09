//! Bounded, order-preserving fan-out (`Effect.forEach(..., { concurrency })`).
//! Futures are boxed before they are buffered so the stream holds no closure
//! types (which keeps the surrounding futures `Send` for every lifetime).

use futures::future::BoxFuture;
use futures::{StreamExt as _, TryStreamExt as _};

/// Runs at most `limit` at once; results in input order.
pub async fn buffered<T>(futures: Vec<BoxFuture<'_, T>>, limit: usize) -> Vec<T> {
    futures::stream::iter(futures)
        .buffered(limit)
        .collect()
        .await
}

/// Like [`buffered`], stopping at the first error.
pub async fn try_buffered<T, E>(
    futures: Vec<BoxFuture<'_, Result<T, E>>>,
    limit: usize,
) -> Result<Vec<T>, E> {
    futures::stream::iter(futures)
        .buffered(limit)
        .try_collect()
        .await
}
