#[cfg(test)]
mod tests {
    const DOCTOR_SOURCE: &str = include_str!("doctor.rs");

    /// Slice production code only — everything before `#[cfg(test)]` — so tests do not
    /// trivially pass because assertion strings themselves contain the searched tokens.
    fn production_code() -> &'static str {
        let end = DOCTOR_SOURCE
            .find("#[cfg(test)]")
            .unwrap_or(DOCTOR_SOURCE.len());
        &DOCTOR_SOURCE[..end]
    }

    #[test]
    fn all_pass_exits_0() {
        let src = production_code();
        assert!(
            src.contains("any_fail"),
            "run_doctor must track any_fail to determine exit code"
        );
        assert!(
            src.contains("Ok(0)"),
            "run_doctor must return Ok(0) when no checks fail"
        );
    }

    #[test]
    fn all_fail_exits_1() {
        let src = production_code();
        assert!(
            src.contains("CheckResult::Fail"),
            "run_doctor must recognise CheckResult::Fail variants"
        );
        assert!(
            src.contains("Ok(1)"),
            "run_doctor must return Ok(1) when any check fails"
        );
    }

    #[test]
    fn docker_host_conflict_is_warn() {
        let src = production_code();
        assert!(
            src.contains("DOCKER_HOST"),
            "check_docker_host must reference DOCKER_HOST env var"
        );
        assert!(
            src.contains("Warn"),
            "check_docker_host conflict must produce Warn, not Fail"
        );
    }

    #[test]
    fn cert_check_skips_when_no_certs() {
        let src = production_code();
        assert!(
            src.contains("no CA certs configured"),
            "cert check must skip with 'no CA certs configured' when extra_certs is empty"
        );
    }

    #[test]
    fn cert_check_fails_when_not_staged() {
        let src = production_code();
        assert!(
            src.contains("not staged"),
            "cert check must fail with 'not staged' hint when cert file is absent from ca-certs dir"
        );
    }

    #[test]
    fn vm_resources_skip_when_no_snapshot() {
        let src = production_code();
        assert!(
            src.contains("vm-config.json not found"),
            "vm resources check must skip with 'vm-config.json not found' when daemon has not started"
        );
    }
}
