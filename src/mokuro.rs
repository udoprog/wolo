use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::header;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use serde::Serialize;
use tokio::fs;

use crate::Error;
use crate::config::Config;
use crate::utils::Templates;

#[derive(Clone)]
struct S {
    templates: Templates,
    config: Arc<Config>,
}

pub(super) fn router(templates: Templates, config: Arc<Config>) -> Router {
    Router::new()
        .route("/", get(list_all))
        .route("/{id}/{name}", get(list_one))
        .route("/{id}/{group}/{name}", get(load))
        .route("/{id}/{group}/{name}/{*key}", get(static_file))
        .with_state(S { templates, config })
}

#[derive(Serialize)]
struct Link {
    title: String,
    href: String,
}

async fn list_all(State(S { templates, config }): State<S>) -> Result<Html<String>, Error> {
    #[derive(Serialize)]
    struct Context {
        links: Vec<Link>,
    }

    let mut links = Vec::new();

    for (n, m) in config.mokuro.iter().enumerate() {
        let mut d = fs::read_dir(&m.path).await?;

        while let Some(d) = d.next_entry().await? {
            let d = d.path();

            let Some(file_name) = d.file_name().and_then(|s| s.to_str()) else {
                continue;
            };

            links.push(Link {
                title: file_name.to_owned(),
                href: format!("/mokuro/{n}/{file_name}"),
            });
        }
    }

    let context = Context { links };

    let o = templates.render("mokuro.html", &context)?;
    Ok(Html(o))
}

async fn list_one(
    State(S { templates, config }): State<S>,
    Path((n, group)): Path<(usize, String)>,
) -> Result<Html<String>, Error> {
    #[derive(Serialize)]
    struct Context {
        links: Vec<Link>,
    }

    let mut links = Vec::new();

    'done: {
        let Some(config) = config.mokuro.get(n) else {
            break 'done;
        };

        let mut d = fs::read_dir(config.path.join(&group)).await?;

        while let Some(d) = d.next_entry().await? {
            let d = d.path();

            if !matches!(d.extension().and_then(|s| s.to_str()), Some("html")) {
                continue;
            }

            let Some(file_name) = d.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };

            links.push(Link {
                title: file_name.to_owned(),
                href: format!("/mokuro/{n}/{group}/{file_name}"),
            });
        }
    };

    let context = Context { links };

    let o = templates.render("mokuro.html", &context)?;
    Ok(Html(o))
}

async fn load(
    State(S { config, .. }): State<S>,
    Path((n, group, name)): Path<(usize, String, String)>,
) -> Result<Html<Vec<u8>>, Error> {
    let Some(config) = config.mokuro.get(n) else {
        return Err(Error::not_found());
    };

    let mut p = config.path.clone();
    p.push(&group);
    p.push(&name);
    p.set_extension("html");

    let bytes = fs::read(&p).await?;
    Ok(Html(bytes))
}

async fn static_file(
    State(S { config, .. }): State<S>,
    Path((n, group, name, rest)): Path<(usize, String, String, String)>,
) -> Result<Response, Error> {
    let Some(config) = config.mokuro.get(n) else {
        return Err(Error::not_found());
    };

    let mut p = config.path.clone();
    p.push(&group);
    p.push(&name);

    for segment in rest.split('/') {
        p.push(segment);
    }

    let mime = mime_guess::from_path(&p).first_or_octet_stream();
    let bytes = fs::read(&p).await?;
    Ok(([(header::CONTENT_TYPE, mime.as_ref())], bytes).into_response())
}
