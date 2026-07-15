use reqwest::Client;
use scraper::{Html, Selector};
use std::time::Duration;

// ─── Engine Config ────────────────────────────────────────────────────────────

struct EngineConfig {
    base_url: &'static str,
    query_param: &'static str,
    extra_params: &'static [(&'static str, &'static str)],
    result_selectors: &'static [&'static str],
    homepage: &'static str,
}

/// Map a string engine name to its scraping configuration.
///
/// Supported names (case-insensitive):
///   Domestic:  baidu, bing_cn, bing_int (alias: bing), 360 (aliases: so360/so),
///              sogou, wechat, shenma
///   Intl:      google, google_hk, duckduckgo (alias: ddg), yahoo, startpage,
///              brave, ecosia, qwant, wolframalpha (aliases: wolfram/wa)
fn engine_config(engine: &str) -> Option<EngineConfig> {
    match engine {
        // ── Domestic ────────────────────────────────────────────────────────
        "baidu" => Some(EngineConfig {
            base_url: "https://www.baidu.com/s",
            query_param: "wd",
            extra_params: &[],
            result_selectors: &[
                "#content_left .c-abstract",
                ".result .c-span-last",
                ".content-right_8Zs40 div",
            ],
            homepage: "https://www.baidu.com",
        }),
        "bing_cn" | "bingcn" | "bing-cn" => Some(EngineConfig {
            base_url: "https://cn.bing.com/search",
            query_param: "q",
            extra_params: &[("ensearch", "0")],
            result_selectors: &[".b_caption p", ".b_snippet", ".b_algoSlug"],
            homepage: "https://cn.bing.com",
        }),
        "bing_int" | "bingint" | "bing-int" | "bing" => Some(EngineConfig {
            base_url: "https://cn.bing.com/search",
            query_param: "q",
            extra_params: &[("ensearch", "1")],
            result_selectors: &[".b_caption p", ".b_snippet", ".b_algoSlug"],
            homepage: "https://cn.bing.com",
        }),
        "360" | "so360" | "so" => Some(EngineConfig {
            base_url: "https://www.so.com/s",
            query_param: "q",
            extra_params: &[],
            result_selectors: &[".res-desc", ".res-desc-text", ".g-excerpt"],
            homepage: "https://www.so.com",
        }),
        "sogou" => Some(EngineConfig {
            base_url: "https://sogou.com/web",
            query_param: "query",
            extra_params: &[],
            result_selectors: &[".star-wiki", ".rb", ".citeurl", ".p-title"],
            homepage: "https://sogou.com",
        }),
        "wechat" => Some(EngineConfig {
            base_url: "https://wx.sogou.com/weixin",
            query_param: "query",
            extra_params: &[("type", "2")],
            result_selectors: &[".txt-info", ".news-text", ".news-list li .txt-box"],
            homepage: "https://wx.sogou.com",
        }),
        "shenma" => Some(EngineConfig {
            base_url: "https://m.sm.cn/s",
            query_param: "q",
            extra_params: &[],
            result_selectors: &[".c-summary", ".c-abstract", ".c-description"],
            homepage: "https://m.sm.cn",
        }),
        // ── International ───────────────────────────────────────────────────
        "google" => Some(EngineConfig {
            base_url: "https://www.google.com/search",
            query_param: "q",
            extra_params: &[],
            result_selectors: &["div.VwiC3b", "span.aCOpRe", "div.IsZvec"],
            homepage: "https://www.google.com",
        }),
        "google_hk" | "googlehk" | "google-hk" => Some(EngineConfig {
            base_url: "https://www.google.com.hk/search",
            query_param: "q",
            extra_params: &[],
            result_selectors: &["div.VwiC3b", "span.aCOpRe", "div.IsZvec"],
            homepage: "https://www.google.com.hk",
        }),
        "duckduckgo" | "ddg" => Some(EngineConfig {
            base_url: "https://html.duckduckgo.com/html/",
            query_param: "q",
            extra_params: &[],
            result_selectors: &[
                ".result__snippet",
                ".result-snippet",
                "td.result-snippet",
                ".snippet",
            ],
            homepage: "https://duckduckgo.com",
        }),
        "yahoo" => Some(EngineConfig {
            base_url: "https://search.yahoo.com/search",
            query_param: "p",
            extra_params: &[],
            result_selectors: &[".d-block.lh-16", ".fc-falcon", ".compText"],
            homepage: "https://www.yahoo.com",
        }),
        "startpage" => Some(EngineConfig {
            base_url: "https://www.startpage.com/sp/search",
            query_param: "query",
            extra_params: &[],
            result_selectors: &[".description", ".search-result__body"],
            homepage: "https://www.startpage.com",
        }),
        "brave" => Some(EngineConfig {
            base_url: "https://search.brave.com/search",
            query_param: "q",
            extra_params: &[],
            result_selectors: &[".snippet-description", ".description"],
            homepage: "https://search.brave.com",
        }),
        "ecosia" => Some(EngineConfig {
            base_url: "https://www.ecosia.org/search",
            query_param: "q",
            extra_params: &[],
            result_selectors: &[".result__description", ".snippet"],
            homepage: "https://www.ecosia.org",
        }),
        "qwant" => Some(EngineConfig {
            base_url: "https://www.qwant.com/",
            query_param: "q",
            extra_params: &[],
            result_selectors: &[
                "[data-testid='result-summary'] span",
                ".result__description",
            ],
            homepage: "https://www.qwant.com",
        }),
        "wolframalpha" | "wolfram" | "wa" => Some(EngineConfig {
            base_url: "https://www.wolframalpha.com/input",
            query_param: "i",
            extra_params: &[],
            result_selectors: &[".pod-content", ".result-pod", "section.pod"],
            homepage: "https://www.wolframalpha.com",
        }),
        _ => None,
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn build_client() -> Result<Client, String> {
    Client::builder()
        .user_agent(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
             AppleWebKit/537.36 (KHTML, like Gecko) \
             Chrome/120.0.0.0 Safari/537.36",
        )
        .cookie_store(true)
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())
}

/// Build an owned URL so it can safely cross `.await` points without
/// holding any borrowed references.
fn build_url(
    base: &str,
    param: &str,
    query: &str,
    extras: &[(&str, &str)],
) -> Result<reqwest::Url, String> {
    let mut url =
        reqwest::Url::parse(base).map_err(|e| format!("Invalid base URL '{}': {}", base, e))?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair(param, query);
        for &(k, v) in extras {
            pairs.append_pair(k, v);
        }
    }
    Ok(url)
}

fn extract_text(html: &str, selectors: &[&str]) -> Option<String> {
    let document = Html::parse_document(html);
    let mut results = String::new();

    for sel_str in selectors {
        if let Ok(selector) = Selector::parse(sel_str) {
            for element in document.select(&selector).take(5) {
                let text = element.text().collect::<Vec<_>>().join(" ");
                let trimmed = text.trim().to_string();
                if !trimmed.is_empty() {
                    results.push_str(&trimmed);
                    results.push('\n');
                }
            }
        }
        if !results.is_empty() {
            break;
        }
    }

    if results.is_empty() {
        None
    } else {
        Some(results)
    }
}

// ─── Fetch with cookie-based retry ───────────────────────────────────────────

/// Fetch search results HTML. On 403/429 responses, acquire fresh session
/// cookies from the engine homepage and retry once after a 2-second delay.
async fn fetch_search_html(
    client: &Client,
    config: &EngineConfig,
    query: &str,
) -> Result<String, String> {
    let url = build_url(
        config.base_url,
        config.query_param,
        query,
        config.extra_params,
    )?;

    let response = client
        .get(url.clone())
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header("Accept-Language", "zh-CN,zh;q=0.9,en-US,en;q=0.8")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let status = response.status();
    if status == reqwest::StatusCode::FORBIDDEN
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        // Acquire fresh session cookies from the homepage, then retry once.
        let _ = client.get(config.homepage).send().await;
        tokio::time::sleep(Duration::from_secs(2)).await;

        return client
            .get(url)
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            )
            .header("Accept-Language", "zh-CN,zh;q=0.9,en-US,en;q=0.8")
            .send()
            .await
            .map_err(|e| e.to_string())?
            .text()
            .await
            .map_err(|e| e.to_string());
    }

    response.text().await.map_err(|e| e.to_string())
}

// ─── DuckDuckGo JSON API ──────────────────────────────────────────────────────

/// Try the DuckDuckGo Instant Answer JSON API first (stable, no key required).
/// Returns `None` if the API returns no useful content.
async fn try_duckduckgo_json(client: &Client, query: &str) -> Option<String> {
    let mut url = reqwest::Url::parse("https://api.duckduckgo.com/").ok()?;
    url.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("format", "json")
        .append_pair("no_html", "1")
        .append_pair("skip_disambig", "1");

    let json = client
        .get(url)
        .send()
        .await
        .ok()?
        .json::<serde_json::Value>()
        .await
        .ok()?;

    let mut results = String::new();

    if let Some(text) = json["AbstractText"].as_str() {
        if !text.is_empty() {
            results.push_str(text);
            results.push('\n');
        }
    }
    if let Some(answer) = json["Answer"].as_str() {
        if !answer.is_empty() {
            results.push_str(answer);
            results.push('\n');
        }
    }
    if let Some(topics) = json["RelatedTopics"].as_array() {
        for topic in topics.iter().take(4) {
            if let Some(text) = topic["Text"].as_str() {
                if !text.is_empty() {
                    results.push_str(text);
                    results.push('\n');
                }
            }
        }
    }

    if results.is_empty() {
        None
    } else {
        Some(results)
    }
}

// ─── Public Tauri Command ─────────────────────────────────────────────────────

/// Search the web using the specified engine and return up to 5 result snippets.
///
/// # Arguments
/// * `engine` – case-insensitive engine name (see [`engine_config`] for the full list)
/// * `query`  – the search query string
///
/// # Supported engines
/// `baidu`, `bing_cn`, `bing_int` / `bing`, `360` / `so360`, `sogou`, `wechat`,
/// `shenma`, `google`, `google_hk`, `duckduckgo` / `ddg`, `yahoo`, `startpage`,
/// `brave`, `ecosia`, `qwant`, `wolframalpha` / `wolfram` / `wa`
#[tauri::command]
pub async fn search_by(engine: String, query: String) -> Result<String, String> {
    let engine_lower = engine.to_lowercase();
    let client = build_client()?;

    // DuckDuckGo: try the richer JSON API before falling back to HTML scraping.
    if engine_lower == "duckduckgo" || engine_lower == "ddg" {
        if let Some(results) = try_duckduckgo_json(&client, &query).await {
            return Ok(results);
        }
    }

    let config = engine_config(&engine_lower).ok_or_else(|| {
        format!(
            "Unknown engine '{}'. Supported: baidu, bing_cn, bing_int, bing, 360, sogou, \
             wechat, shenma, google, google_hk, duckduckgo, ddg, yahoo, startpage, \
             brave, ecosia, qwant, wolframalpha, wolfram, wa",
            engine
        )
    })?;

    let html = fetch_search_html(&client, &config, &query).await?;

    extract_text(&html, config.result_selectors)
        .ok_or_else(|| format!("'{}' returned no results for: {}", engine, query))
}
