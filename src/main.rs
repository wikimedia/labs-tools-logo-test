/*
Easily test new logos on Wikimedia sites
Copyright (C) 2021 Kunal Mehta <legoktm@member.fsf.org>

This program is free software: you can redistribute it and/or modify
it under the terms of the GNU Affero General Public License as published by
the Free Software Foundation, either version 3 of the License, or
(at your option) any later version.

This program is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
GNU Affero General Public License for more details.

You should have received a copy of the GNU Affero General Public License
along with this program.  If not, see <http://www.gnu.org/licenses/>.
*/

use anyhow::{anyhow, Result};
use regex::Regex;
use rocket::response::content;
use rocket_contrib::templates::Template;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[macro_use]
extern crate rocket;

const USER_AGENT: &str = toolforge::user_agent!("logo-test");

/// CSS copied from MediaWiki's output
const CSS: &str = r#"
<style type="text/css">
.mw-wiki-logo {
 background-image:url($logo)
}

@media (-webkit-min-device-pixel-ratio:1.5),(min--moz-device-pixel-ratio:1.5),(min-resolution:1.5dppx),(min-resolution:144dpi) {
 .mw-wiki-logo {
  background-image:url($logo_1_5x);
  background-size:135px auto
 }
}
@media (-webkit-min-device-pixel-ratio:2),(min--moz-device-pixel-ratio:2),(min-resolution:2dppx),(min-resolution:192dpi) {
 .mw-wiki-logo {
  background-image:url($logo_2x);
  background-size:135px auto;
 }
}
</style>
</head>
"#;

#[derive(Serialize, Deserialize)]
struct ErrorTemplate {
    error: String,
}

/// Build a HTTP client
fn client() -> Result<reqwest::Client> {
    Ok(reqwest::ClientBuilder::new()
        .user_agent(USER_AGENT)
        .build()?)
}

#[get("/?<wiki>&<logo>")]
fn index(wiki: Option<String>, logo: Option<String>) -> Template {
    match build_index(wiki, logo) {
        Ok(index) => Template::render("main", index),
        Err(err) => {
            dbg!(&err);
            Template::render(
                "error",
                ErrorTemplate {
                    error: err.to_string(),
                },
            )
        }
    }
}

#[derive(Serialize)]
struct IndexTemplate {
    wiki: Option<String>,
    logo: Option<String>,
}

/// Build the index template (`/`)
fn build_index(wiki: Option<String>, logo: Option<String>) -> Result<IndexTemplate> {
    if let Some(wiki) = &wiki {
        validate_wiki(wiki)?;
    }
    if let Some(logo) = &logo {
        validate_logo(logo)?;
    }
    Ok(IndexTemplate { wiki, logo })
}

#[get("/test?<wiki>&<logo>&<useskin>")]
async fn test(
    wiki: String,
    logo: String,
    useskin: String,
) -> Result<content::Html<String>, Template> {
    match build_test(&wiki, &logo, &useskin).await {
        Ok(text) => Ok(content::Html(text)),
        Err(err) => {
            dbg!(&err);
            Err(Template::render(
                "error",
                ErrorTemplate {
                    error: err.to_string(),
                },
            ))
        }
    }
}

#[derive(Deserialize)]
struct ImageInfo {
    thumburl: String,
    #[serde(rename = "responsiveUrls")]
    responsive_urls: ResponsiveUrls,
}

#[derive(Deserialize)]
struct ResponsiveUrls {
    #[serde(rename = "1.5")]
    one_half: String,
    #[serde(rename = "2")]
    two: String,
}

fn validate_skin(skin: &str) -> Result<()> {
    if vec!["vector", "timeless", "monobook"].contains(&skin) {
        Ok(())
    } else {
        Err(anyhow!("Invalid skin specified"))
    }
}

fn validate_wiki(wiki: &str) -> Result<()> {
    use mysql::prelude::*;
    use mysql::*;
    let db_url = toolforge::connection_info!("enwiki", WEB)?;
    let pool = Pool::new(db_url.to_string())?;
    let mut conn = pool.get_conn()?;
    let full_wiki = format!("https://{}", wiki);
    let resp: Option<u32> =
        conn.exec_first("SELECT 1 FROM meta_p.wiki WHERE url = ?", (full_wiki,))?;
    // TODO: do we need to explicitly close the connection?
    if resp.is_some() {
        Ok(())
    } else {
        Err(anyhow!("Invalid wiki"))
    }
}

fn validate_logo(logo: &str) -> Result<()> {
    if !logo.ends_with(".svg") {
        Err(anyhow!("Logo must be a SVG"))
    } else if !logo.starts_with("File:") {
        Err(anyhow!("Logo must begin with File:"))
    } else {
        Ok(())
    }
}

/// Fetch thumbs from Commons and turn it into CSS
async fn commons_thumbs(logo: &str) -> Result<String> {
    let resp = client()?.get(
        &format!("https://commons.wikimedia.org/w/api.php?action=query&format=json&prop=imageinfo&titles={}&formatversion=2&iiprop=url&iiurlwidth=135", logo)
    ).send().await?;

    let data: Value = resp.json().await?;
    dbg!(&data);
    let info: ImageInfo =
        serde_json::from_value(data["query"]["pages"][0]["imageinfo"][0].clone())?;
    // Replace the URLs in:
    let css = CSS
        .to_string()
        .replace("$logo_1_5x", &info.responsive_urls.one_half)
        .replace("$logo_2x", &info.responsive_urls.two)
        .replace("$logo", &info.thumburl);
    Ok(css)
}

async fn build_test(wiki: &str, logo: &str, useskin: &str) -> Result<String> {
    validate_skin(useskin)?;
    validate_wiki(wiki)?;
    validate_logo(logo)?;
    let resp = client()?
        .get(&format!("https://{}/?useskin={}", wiki, useskin))
        .send()
        .await?;
    let text = resp.text().await?;

    // Make some URLs absolute
    let re = Regex::new(r#"(?P<attr>(src|href))="/(?P<letter>[A-z])"#).unwrap();
    let rep = format!(r#"$attr="//{}/$letter"#, wiki);
    let fixed = re.replace_all(&text, rep.as_str());

    // Inject the Commmons logo CSS
    let css = commons_thumbs(logo).await?;
    let injected = fixed.replace("</head>", &css);
    Ok(injected)
}

#[launch]
fn rocket() -> rocket::Rocket {
    rocket::ignite()
        .attach(Template::fairing())
        .mount("/", routes![index, test])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_commons_thumbs() {
        let resp = commons_thumbs("File:Wikipedia-logo-v2-wordmark.svg")
            .await
            .unwrap();
        assert_eq!(
            &resp,
            r#"
<style type="text/css">
.mw-wiki-logo {
 background-image:url(https://upload.wikimedia.org/wikipedia/commons/thumb/f/f6/Wikipedia-logo-v2-wordmark.svg/135px-Wikipedia-logo-v2-wordmark.svg.png)
}

@media (-webkit-min-device-pixel-ratio:1.5),(min--moz-device-pixel-ratio:1.5),(min-resolution:1.5dppx),(min-resolution:144dpi) {
 .mw-wiki-logo {
  background-image:url(https://upload.wikimedia.org/wikipedia/commons/thumb/f/f6/Wikipedia-logo-v2-wordmark.svg/203px-Wikipedia-logo-v2-wordmark.svg.png);
  background-size:135px auto
 }
}
@media (-webkit-min-device-pixel-ratio:2),(min--moz-device-pixel-ratio:2),(min-resolution:2dppx),(min-resolution:192dpi) {
 .mw-wiki-logo {
  background-image:url(https://upload.wikimedia.org/wikipedia/commons/thumb/f/f6/Wikipedia-logo-v2-wordmark.svg/270px-Wikipedia-logo-v2-wordmark.svg.png);
  background-size:135px auto;
 }
}
</style>
</head>
"#
        );
    }

    #[test]
    fn test_validate_skin() {
        // No panic
        validate_skin("vector").unwrap()
    }

    #[test]
    #[should_panic]
    fn test_validate_skin_bad() {
        validate_skin("whatever").unwrap();
    }

    #[test]
    fn test_validate_logo() {
        // No panic
        validate_logo("File:Wiki.svg").unwrap();
    }

    #[test]
    #[should_panic]
    fn test_validate_logo_no_file() {
        validate_logo("Wiki.svg").unwrap();
    }

    #[test]
    #[should_panic]
    fn test_validate_logo_not_svg() {
        validate_logo("File:Wiki.png").unwrap();
    }
}
