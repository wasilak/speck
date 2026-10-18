//! Human-readable sizes for VM memory and disk: `6g`, `6G`, `6GiB`,
//! `6144m`, `1.5g`. Units are binary (1g = 1024m), like `docker run -m`.
//! A plain number keeps its old meaning: MiB for memory, GiB for disk.

const KIB: f64 = 1024.0;

/// Bytes for `input`, where a number without a unit is `default_unit` bytes.
fn parse_bytes(input: &str, default_unit: f64) -> Result<f64, String> {
    let s = input.trim();
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (number, unit) = s.split_at(split);
    let value: f64 = number
        .parse()
        .map_err(|_| format!("invalid size `{input}`; use e.g. 6g, 6144m or 1.5g"))?;
    let multiplier = match unit.trim().to_ascii_lowercase().as_str() {
        "" => default_unit,
        "k" | "kb" | "kib" => KIB,
        "m" | "mb" | "mib" => KIB * KIB,
        "g" | "gb" | "gib" => KIB * KIB * KIB,
        "t" | "tb" | "tib" => KIB * KIB * KIB * KIB,
        other => {
            return Err(format!(
                "unknown size unit `{other}` in `{input}`; use k, m, g or t"
            ));
        }
    };
    Ok(value * multiplier)
}

fn whole(bytes: f64, unit: f64, unit_name: &str, input: &str) -> Result<u64, String> {
    let n = bytes / unit;
    if n.fract() != 0.0 {
        return Err(format!("`{input}` is not a whole number of {unit_name}"));
    }
    Ok(n as u64)
}

/// Memory size in MiB. Plain numbers are MiB.
pub fn parse_mib(input: &str) -> Result<u64, String> {
    let bytes = parse_bytes(input, KIB * KIB)?;
    whole(bytes, KIB * KIB, "MiB", input)
}

/// Disk size in GiB. Plain numbers are GiB.
pub fn parse_gib(input: &str) -> Result<u64, String> {
    let bytes = parse_bytes(input, KIB * KIB * KIB)?;
    whole(bytes, KIB * KIB * KIB, "GiB", input)
}

/// serde helpers: accept a number (old format) or a size string.
pub mod serde_size {
    use serde::{Deserialize, Deserializer};

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum NumberOrSize {
        Number(u64),
        Size(String),
    }

    fn parse_with(
        value: Option<NumberOrSize>,
        parse: fn(&str) -> Result<u64, String>,
    ) -> Result<Option<u64>, String> {
        match value {
            None => Ok(None),
            Some(NumberOrSize::Number(n)) => Ok(Some(n)),
            Some(NumberOrSize::Size(s)) => parse(&s).map(Some),
        }
    }

    pub fn mib<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
        let value = Option::<NumberOrSize>::deserialize(d)?;
        parse_with(value, super::parse_mib)
            .map_err(|e| serde::de::Error::custom(format!("vm.memory_mb: {e}")))
    }

    pub fn gib<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
        let value = Option::<NumberOrSize>::deserialize(d)?;
        parse_with(value, super::parse_gib)
            .map_err(|e| serde::de::Error::custom(format!("vm.disk_gb: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_accepts_units_in_any_case() {
        for input in ["6g", "6G", "6gb", "6GiB", "6144m", "6144M", "6144", " 6 g "] {
            assert_eq!(parse_mib(input), Ok(6144), "{input}");
        }
        assert_eq!(parse_mib("1.5g"), Ok(1536));
        assert_eq!(parse_mib("1t"), Ok(1024 * 1024));
    }

    #[test]
    fn disk_accepts_units_and_plain_gib() {
        assert_eq!(parse_gib("100"), Ok(100));
        assert_eq!(parse_gib("100G"), Ok(100));
        assert_eq!(parse_gib("1t"), Ok(1024));
        assert_eq!(parse_gib("2048m"), Ok(2));
    }

    #[test]
    fn rejects_garbage_and_fractions() {
        assert!(parse_mib("abc").is_err());
        assert!(parse_mib("6x").is_err());
        assert!(parse_mib("").is_err());
        assert!(parse_mib("1.5").is_err());
        assert!(parse_gib("1.5g").is_err());
        assert!(parse_mib("100k").is_err());
    }
}
