use core::str;

use std::sync::Arc;

use anyhow::Error;
use minijinja::Environment;
use relative_path::RelativePath;
use serde::Serialize;

use crate::embed::Assets;

/// Handler for templates.
#[derive(Clone)]
pub(crate) struct Templates {
    env: Arc<Environment<'static>>,
}

impl Templates {
    /// Render a template by name.
    pub(crate) fn render(&self, name: &str, context: impl Serialize) -> Result<String, Error> {
        let template = self.env.get_template(name)?;
        let rendered = template.render(context)?;
        Ok(rendered)
    }
}

pub(crate) fn load_templates() -> Result<Templates, Error> {
    let mut env = Environment::new();
    env.set_keep_trailing_newline(false);
    env.set_trim_blocks(true);
    env.set_lstrip_blocks(true);

    for name in Assets::iter() {
        let path = RelativePath::new(name.as_ref());

        let Some(content) = Assets::get(path.as_str()) else {
            continue;
        };

        if path.extension() != Some("html") {
            continue;
        }

        let Ok(content) = str::from_utf8(content.data.as_ref()) else {
            continue;
        };

        env.add_template_owned(path.as_str().to_owned(), content.to_owned())?;
    }

    env.add_filter("hex", |value: u16| Ok(format!("0x{:x}", value)));
    Ok(Templates { env: Arc::new(env) })
}
