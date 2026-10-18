//! `-o plain|json|yaml`: machine-readable output for the reporting commands.
//! In json/yaml mode stdout holds only the document; warnings go to stderr.

use serde::Serialize;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Output {
    #[default]
    Plain,
    Json,
    Yaml,
}

impl Output {
    pub fn is_plain(self) -> bool {
        self == Output::Plain
    }

    /// `-j/--json` (kept from before `-o`) as an alias for `-o json`.
    pub fn or_json(self, json: bool) -> Self {
        if json { Output::Json } else { self }
    }
}

/// The document as JSON or YAML text, with a trailing newline.
pub fn render<T: Serialize>(format: Output, value: &T) -> anyhow::Result<String> {
    Ok(match format {
        Output::Json | Output::Plain => serde_json::to_string_pretty(value)? + "\n",
        Output::Yaml => serde_yaml::to_string(value)?,
    })
}

/// Print `value` in `format`, which must not be `Plain`.
pub fn emit<T: Serialize>(format: Output, value: &T) -> anyhow::Result<()> {
    debug_assert!(!format.is_plain());
    anstream::print!("{}", render(format, value)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct Doc {
        name: &'static str,
        cpus: u64,
    }

    #[test]
    fn json_and_yaml_carry_the_same_document() {
        let doc = Doc {
            name: "work",
            cpus: 2,
        };
        let json: serde_json::Value =
            serde_json::from_str(&render(Output::Json, &doc).unwrap()).unwrap();
        let yaml: serde_json::Value =
            serde_yaml::from_str(&render(Output::Yaml, &doc).unwrap()).unwrap();
        assert_eq!(json, yaml);
        assert_eq!(json["cpus"], 2);
    }

    #[test]
    fn json_flag_is_an_alias() {
        assert_eq!(Output::Plain.or_json(true), Output::Json);
        assert_eq!(Output::Yaml.or_json(false), Output::Yaml);
    }
}
