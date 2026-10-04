//! RAR decoder compatibility facts gathered during the SLT listing pass.

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RarCompatibility {
    pub(crate) rar7_v6_confirmed_unencrypted: bool,
    pub(crate) legacy_p7zip_rar5_decoder_gap: bool,
}

#[derive(Default)]
struct EntryEvidence {
    has_path: bool,
    has_entry_field: bool,
    matches_method: bool,
    explicitly_unencrypted: bool,
}

#[derive(Default)]
struct CompatibilityEvidence {
    block: EntryEvidence,
    rejected: bool,
    matched_entry: bool,
}

#[derive(Default)]
pub(super) struct RarFacts {
    legacy_p7zip: bool,
    rar5: bool,
    v6: CompatibilityEvidence,
    legacy: CompatibilityEvidence,
}

impl RarFacts {
    pub(super) fn observe_line(&mut self, line: &str, field: Option<(&str, &str)>) {
        self.legacy_p7zip |= line.trim_start().starts_with("p7zip Version 16.02 ");
        self.rar5 |= line.trim() == "Type = Rar5";
        let Some((key, value)) = field else {
            return;
        };
        let value = value.trim();

        // The v6 probe uses literal keys and keeps any v6 method in a block.
        match key {
            "Path" => self.v6.block.has_path = !value.is_empty(),
            "Folder" | "Size" | "Packed Size" | "Attributes" | "CRC" => {
                self.v6.block.has_entry_field = true;
            }
            "Method" if value.starts_with("v6:") => self.v6.block.matches_method = true,
            "Encrypted" if value == "-" => self.v6.block.explicitly_unencrypted = true,
            "Encrypted" => self.v6.rejected = true,
            _ => {}
        }

        // The p7zip probe trims keys and uses the last method in each block.
        match key.trim() {
            "Path" => self.legacy.block.has_path = !value.is_empty(),
            "Folder" | "Size" | "Packed Size" | "Attributes" | "CRC" => {
                self.legacy.block.has_entry_field = true;
            }
            "Method" => {
                self.legacy.block.matches_method = value
                    .strip_prefix('m')
                    .and_then(|method| method.as_bytes().first())
                    .is_some_and(|level| matches!(level, b'1'..=b'5'));
            }
            "Encrypted" if value == "-" => self.legacy.block.explicitly_unencrypted = true,
            "Encrypted" => self.legacy.rejected = true,
            "Symbolic Link" | "Hard Link" | "Copy Link" if !value.is_empty() => {
                self.legacy.rejected = true;
            }
            _ => {}
        }
    }

    pub(super) fn finish_block(&mut self) {
        for evidence in [&mut self.v6, &mut self.legacy] {
            let block = std::mem::take(&mut evidence.block);
            if block.has_path && block.has_entry_field {
                evidence.rejected |= !block.explicitly_unencrypted;
                evidence.matched_entry |= block.matches_method;
            }
        }
    }

    pub(super) fn finish(self) -> RarCompatibility {
        RarCompatibility {
            rar7_v6_confirmed_unencrypted: self.v6.matched_entry && !self.v6.rejected,
            legacy_p7zip_rar5_decoder_gap: self.legacy_p7zip
                && self.rar5
                && self.legacy.matched_entry
                && !self.legacy.rejected,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RarCompatibility, RarFacts};

    fn facts(text: &str) -> RarCompatibility {
        let mut facts = RarFacts::default();
        for line in text.lines() {
            if line.is_empty() {
                facts.finish_block();
            } else {
                facts.observe_line(line, line.split_once(" = "));
            }
        }
        facts.finish_block();
        facts.finish()
    }

    #[test]
    fn rar7_v6_detection_requires_positive_unencrypted_evidence() {
        let confirmed = "Path = hello.txt\nSize = 5\nMethod = v6:m3:128K\nEncrypted = -\n";
        assert!(facts(confirmed).rar7_v6_confirmed_unencrypted);
        assert!(facts(confirmed.trim_end()).rar7_v6_confirmed_unencrypted);
        assert!(facts(&confirmed.replace('\n', "\r\n")).rar7_v6_confirmed_unencrypted);
        for text in [
            "Path = hello.txt\nSize = 5\nMethod = v6:m3:128K\n",
            "Path = hello.txt\nMethod = v6:m3:128K\nEncrypted = -\n",
            "Path = hello.txt\nSize = 5\nMethod = m5:128K\nEncrypted = -\n",
            " Path = hello.txt\nSize = 5\nMethod = v6:m3:128K\nEncrypted = -\n",
            "Path = hello.txt\nSize = 5\n Method = v6:m3:128K\nEncrypted = -\n",
            "Path = hello.txt\nSize = 5\nMethod = v6:m3:128K\n Encrypted = -\n",
            "Path = hello.txt\nPath = \nSize = 5\nMethod = v6:m3:128K\nEncrypted = -\n",
        ] {
            assert!(!facts(text).rar7_v6_confirmed_unencrypted, "{text:?}");
        }
        for tail in [
            "\nPath = secret.txt\nSize = 6\nMethod = v6:m3:128K\nEncrypted = +\n",
            "\nPath = unknown.txt\nSize = 7\nMethod = v6:m3:128K\n",
            "Encrypted = +\nEncrypted = -\n",
            "Encrypted = ?\n",
            "\nEncrypted = +\n",
        ] {
            assert!(
                !facts(&format!("{confirmed}{tail}")).rar7_v6_confirmed_unencrypted,
                "{tail:?}"
            );
        }
        assert!(!facts(&format!("Encrypted = +\n\n{confirmed}")).rar7_v6_confirmed_unencrypted);
        assert!(facts(&format!("{confirmed}Method = m0\n")).rar7_v6_confirmed_unencrypted);
        assert!(facts(&format!("{confirmed} Encrypted = +\n")).rar7_v6_confirmed_unencrypted);
        for key in ["Symbolic Link", "Hard Link", "Copy Link"] {
            assert!(
                facts(&format!("{confirmed}{key} = target.txt\n")).rar7_v6_confirmed_unencrypted
            );
        }
    }

    #[test]
    fn legacy_p7zip_rar5_detection_is_narrow() {
        let compressed_rar5 = "7-Zip [64] 16.02\n\
            p7zip Version 16.02 (locale=C)\n\n\
            Path = archive.rar\nType = Rar5\nPhysical Size = 100\n\n\
            Path = hello.txt\nSize = 5\nMethod = m5:17\nEncrypted = -\n";
        assert!(facts(compressed_rar5).legacy_p7zip_rar5_decoder_gap);
        assert!(facts(compressed_rar5.trim_end()).legacy_p7zip_rar5_decoder_gap);
        assert!(facts(&compressed_rar5.replace('\n', "\r\n")).legacy_p7zip_rar5_decoder_gap);
        for (from, to) in [
            ("p7zip Version 16.02", "7-Zip 16.02"),
            ("Type = Rar5", "Type = Rar"),
            ("Type = Rar5", "Type =  Rar5"),
            ("Method = m5:17", "Method = m0"),
            ("Encrypted = -", "Encrypted = +"),
            ("Encrypted = -\n", ""),
            ("Size = 5\n", ""),
            ("Path = hello.txt", "Path = "),
        ] {
            let text = compressed_rar5.replace(from, to);
            assert!(!facts(&text).legacy_p7zip_rar5_decoder_gap, "{text:?}");
        }
        assert!(
            facts(&compressed_rar5.replace("Path = hello.txt", " Path = hello.txt"))
                .legacy_p7zip_rar5_decoder_gap
        );
        assert!(
            facts(&compressed_rar5.replace("Method = m5:17", " Method = m5:17"))
                .legacy_p7zip_rar5_decoder_gap
        );
        assert!(
            facts(&compressed_rar5.replace("Encrypted = -", " Encrypted = -"))
                .legacy_p7zip_rar5_decoder_gap
        );
        assert!(!facts(&format!("{compressed_rar5}Method = m0\n")).legacy_p7zip_rar5_decoder_gap);
        assert!(
            facts(&format!(
                "{}Method = m5:17\n",
                compressed_rar5.replace("m5:17", "m0")
            ))
            .legacy_p7zip_rar5_decoder_gap
        );
        for tail in [
            "Encrypted = +\nEncrypted = -\n",
            "\nPath = unknown.txt\nSize = 1\n",
            "\nEncrypted = +\n",
            " Encrypted = +\n",
            "Path = \n",
        ] {
            assert!(
                !facts(&format!("{compressed_rar5}{tail}")).legacy_p7zip_rar5_decoder_gap,
                "{tail:?}"
            );
        }
        for key in ["Symbolic Link", "Hard Link", "Copy Link"] {
            assert!(
                !facts(&format!("{compressed_rar5}{key} = target.txt\n"))
                    .legacy_p7zip_rar5_decoder_gap
            );
            assert!(facts(&format!("{compressed_rar5}{key} = \n")).legacy_p7zip_rar5_decoder_gap);
        }
    }
}
