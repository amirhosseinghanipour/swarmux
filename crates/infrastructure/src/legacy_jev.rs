//! What the removed JEV may have left on the machine outside swarmux's own
//! files: an API key in the OS keychain. swarmux never deletes it by itself
//! (it is the user's secret, and may be shared with other tools); it only
//! says whether one is there and how to remove it.

/// Keychain service name the old code stored its keys under.
pub const KEYCHAIN_SERVICE: &str = "swarmux";

/// The keychain accounts it used: one per provider.
pub const KEYCHAIN_ACCOUNTS: [&str; 2] = ["typesafe", "openrouter"];

/// The accounts that still hold a key. Reads each entry only to tell it is
/// there and drops the value at once; nothing is printed or kept.
pub fn stored_accounts() -> Vec<&'static str> {
    KEYCHAIN_ACCOUNTS
        .into_iter()
        .filter(|account| {
            keyring::Entry::new(KEYCHAIN_SERVICE, account)
                .and_then(|entry| entry.get_password())
                .is_ok()
        })
        .collect()
}

/// How to remove the key of `account` from the OS keychain, one line per
/// system. swarmux runs none of these.
pub fn removal_steps(account: &str) -> Vec<String> {
    vec![
        format!(
            "Windows: cmdkey /delete:{account}.{KEYCHAIN_SERVICE}  (or Credential Manager > Windows Credentials > \"{account}.{KEYCHAIN_SERVICE}\")"
        ),
        format!("macOS:   security delete-generic-password -s {KEYCHAIN_SERVICE} -a {account}"),
        format!(
            "Linux:   id=$(keyctl search @s user \"keyring-rs:{account}@{KEYCHAIN_SERVICE}\") && keyctl unlink \"$id\" @s && keyctl unlink \"$id\" @p"
        ),
    ]
}

/// The doctor's lines about a key left behind, empty when there is none.
/// `TYPESAFE_API_KEY` and `OPENROUTER_API_KEY` in the environment are not
/// swarmux's to remove and are not mentioned.
pub fn doctor_lines() -> Vec<String> {
    doctor_lines_for(&stored_accounts())
}

/// The lines for the keychain accounts in `found` (none: no lines).
pub fn doctor_lines_for(found: &[&str]) -> Vec<String> {
    if found.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![format!(
        "warn a JEV key is still in the OS keychain (service \"{KEYCHAIN_SERVICE}\", account {}); swarmux no longer uses it and does not remove it. Remove it yourself if you don't need it:",
        found
            .iter()
            .map(|a| format!("\"{a}\""))
            .collect::<Vec<_>>()
            .join(" and ")
    )];
    for account in found {
        for step in removal_steps(account) {
            lines.push(format!("       {step}"));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_names_are_the_ones_the_old_code_used() {
        assert_eq!(KEYCHAIN_SERVICE, "swarmux");
        assert_eq!(KEYCHAIN_ACCOUNTS, ["typesafe", "openrouter"]);
    }

    #[test]
    fn nothing_is_said_when_no_key_is_left_and_the_account_is_named_when_one_is() {
        assert!(doctor_lines_for(&[]).is_empty());
        let lines = doctor_lines_for(&["typesafe"]);
        println!(
            "{}",
            lines.join(
                "
"
            )
        );
        assert!(lines[0].starts_with("warn a JEV key is still in the OS keychain"));
        assert!(lines[0].contains("service \"swarmux\", account \"typesafe\""));
        assert!(lines[0].contains("does not remove it"));
        assert_eq!(lines.len(), 4, "the warning and one line per system");
        // Only the accounts that hold a key are mentioned: never the
        // ledger's signing key, which lives under the same service.
        assert!(
            !lines
                .join(
                    "
"
                )
                .contains("audit-signing-key")
        );
        assert!(
            !lines
                .join(
                    "
"
                )
                .contains("openrouter")
        );
    }

    #[test]
    fn removal_steps_name_service_and_account_for_every_system() {
        let steps = removal_steps("typesafe").join("\n");
        assert!(steps.contains("cmdkey /delete:typesafe.swarmux"), "{steps}");
        assert!(
            steps.contains("security delete-generic-password -s swarmux -a typesafe"),
            "{steps}"
        );
        assert!(steps.contains("keyring-rs:typesafe@swarmux"), "{steps}");
    }
}
