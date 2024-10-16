use derive_more::derive::Display;
use futures::future::join_all;
use rayon::prelude::*;
use serde::*;
use thiserror::Error;
pub trait RemoteScrapable: Send + Sync {
    type Output;
    /// The url formatted for a given id
    fn id_url(&self, id: &String) -> String;
    /// Transform the element into [Self::Output]
    fn elem_into<'a>(
        &self,
        elem: &'a scraper::ElementRef<'a>,
    ) -> Result<Self::Output, RemoteScrapeError>;
    /// Resource selector
    fn res_selector(&self) -> &'static str;
}
#[derive(Error, Debug, Display, Clone, Serialize, Deserialize)]
pub enum RemoteScrapeError {
    // Reqwest errror not clonable
    Request(String),
    UrlParse(String),
    NoElementMatch,
    PageNotFound(String),
}
pub async fn remote_scrape<T: RemoteScrapable>(
    remote_scrappable: &T,
    ids: &Vec<String>,
) -> Vec<Result<T::Output, RemoteScrapeError>> {
    let client = reqwest::Client::builder().build().unwrap();
    join_all(
        ids.par_iter()
            .map(|id| async {
                let id_url = remote_scrappable.id_url(id);
                let response = client
                    .get(id_url.clone())
                    .send()
                    .await
                    .map_err(|e| RemoteScrapeError::Request(format!("{e}")))?;
                if response.status() == 404 {
                    // https://crates.io/crates/analiticcl & https://docs.rs/zspell/latest/zspell/
                    return Err(RemoteScrapeError::PageNotFound(id_url));
                }
                let response_html_text = response
                    .text()
                    .await
                    .map_err(|e| RemoteScrapeError::Request(format!("{e}")))?;

                let selector = scraper::Selector::parse(&remote_scrappable.res_selector()).unwrap();
                let document = scraper::Html::parse_document(&response_html_text);
                let elems = document.select(&selector).collect::<Vec<_>>();
                match elems.first() {
                    Some(elem) => remote_scrappable.elem_into(elem),
                    None => Err(RemoteScrapeError::NoElementMatch),
                }
            })
            .collect::<Vec<_>>(),
    )
    .await
}
