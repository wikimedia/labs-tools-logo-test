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
use rocket_dyn_templates::Template;
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

#[derive(Serialize)]
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
        validate_domain(wiki)?;
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

fn validate_domain(wiki: &str) -> Result<()> {
    use mysql::prelude::*;
    use mysql::*;
    let domain = if wiki.starts_with("https://") {
        let parsed = url::Url::parse(wiki)?;
        match parsed.host_str() {
            Some(domain) => domain.to_string(),
            None => return Err(anyhow!("Invalid domain specified")),
        }
    } else {
        wiki.to_string()
    };
    if domain == "upload.wikimedia.org" || domain == "people.wikimedia.org" {
        // Non-wiki, safe domains
        return Ok(());
    }
    let db_url = match toolforge::connection_info!("meta_p", WEB) {
        Ok(info) => info.to_string(),
        // If we're not on Toolforge, don't bother validating
        Err(toolforge::Error::NotToolforge(_)) => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    let pool = Pool::new(db_url)?;
    let mut conn = pool.get_conn()?;
    let full_domain = format!("https://{}", domain);
    let resp: Option<u32> = conn.exec_first("SELECT 1 FROM wiki WHERE url = ?", (full_domain,))?;
    // TODO: do we need to explicitly close the connection?
    if resp.is_some() {
        Ok(())
    } else {
        Err(anyhow!("Invalid domain"))
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
        .replace(
            "$logo_1_5x",
            &info.responsive_urls.one_half.replace("203", "202"),
        )
        .replace("$logo_2x", &info.responsive_urls.two)
        .replace("$logo", &info.thumburl);
    Ok(css)
}

async fn build_test(wiki: &str, logo: &str, useskin: &str) -> Result<String> {
    validate_skin(useskin)?;
    validate_domain(wiki)?;
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

#[derive(Serialize)]
struct DiffTemplate {
    logo1: Option<String>,
    logo2: Option<String>,
    logo1_safe: Option<String>,
    logo2_safe: Option<String>,
}

#[get("/diff?<logo1>&<logo2>")]
fn diff(logo1: Option<String>, logo2: Option<String>) -> Template {
    match build_diff(logo1, logo2) {
        Ok(diff) => Template::render("diff", diff),
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

/// Build the diff template (`/`)
fn build_diff(logo1: Option<String>, logo2: Option<String>) -> Result<DiffTemplate> {
    let logo1_safe = if let Some(logo1) = &logo1 {
        validate_domain(logo1)?;
        Some(serde_json::to_string(logo1)?)
    } else {
        None
    };
    let logo2_safe = if let Some(logo2) = &logo2 {
        validate_domain(logo2)?;
        Some(serde_json::to_string(logo2)?)
    } else {
        None
    };
    Ok(DiffTemplate {
        logo1,
        logo2,
        logo1_safe,
        logo2_safe,
    })
}

#[get("/healthz")]
fn healthz() -> &'static str {
    "OK"
}

#[launch]
fn rocket() -> _ {
    rocket::build()
        .attach(Template::fairing())
        .mount("/", routes![index, diff, healthz, test])
}

#[cfg(test)]
mod tests {
    use super::*;
    use rocket::http::Status;
    use rocket::local::blocking::Client;

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
  background-image:url(https://upload.wikimedia.org/wikipedia/commons/thumb/f/f6/Wikipedia-logo-v2-wordmark.svg/202px-Wikipedia-logo-v2-wordmark.svg.png);
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

    #[test]
    fn test_index() {
        let client = Client::tracked(rocket()).unwrap();
        let response = client.get("/").dispatch();
        assert_eq!(response.status(), Status::Ok);
        assert!(response
            .into_string()
            .unwrap()
            .contains("The logo-test tool allows you"));

        let response = client
            .get("/?wiki=en.wikipedia.org&logo=File%3AUncyclomedia+blue+logo+notext.svg")
            .dispatch();
        assert_eq!(response.status(), Status::Ok);
        assert!(response
            .into_string()
            .unwrap()
            .contains("Using the vector skin"));

        // Error handling
        let response = client
            .get("/?wiki=en.wikipedia.org&logo=Bad_logo")
            .dispatch();
        assert_eq!(response.status(), Status::Ok);
        assert!(response.into_string().unwrap().contains("logo-test: error"))
    }

    #[test]
    fn test_test() {
        // the /test endpoint
        let client = Client::tracked(rocket()).unwrap();
        let response = client.get("/test").dispatch();
        assert_eq!(response.status(), Status::NotFound);

        let response = client
            .get("/test?wiki=en.wikipedia.org&logo=File%3AUncyclomedia+blue+logo+notext.svg&useskin=timeless")
            .dispatch();
        assert_eq!(response.status(), Status::Ok);
        assert!(response
            .into_string()
            .unwrap()
            // the 2x variant, good enough for an integration test
            .contains("270px-Uncyclomedia_blue_logo_notext.svg.png"));

        // Error handling
        let response = client
            .get("/test?wiki=en.wikipedia.org&logo=Bad_logo&useskin=timeless")
            .dispatch();
        assert_eq!(response.status(), Status::Ok);
        assert!(response.into_string().unwrap().contains("logo-test: error"))
    }

    #[test]
    fn test_validate_domain() {
        validate_domain("upload.wikimedia.org").unwrap();
        validate_domain("people.wikmedia.org").unwrap();
        // TODO: why is this failing?
        // assert!(validate_domain("/foo/bar").err().is_some());
    }
}
