use std::sync::LazyLock;

/// A re-usable [`reqwest::Client`] for use in HTTP requests.
pub static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(reqwest::Client::new);
