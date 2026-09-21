use std::collections::BTreeMap;

const EN_US: &str = include_str!("../resources/en-US.ftl");
const ZH_CN: &str = include_str!("../resources/zh-CN.ftl");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    English,
    SimplifiedChinese,
}

impl Language {
    #[must_use]
    pub fn from_locale_tag(tag: &str) -> Self {
        if tag
            .split(['-', '_', '.'])
            .next()
            .is_some_and(|language| language.eq_ignore_ascii_case("zh"))
        {
            Self::SimplifiedChinese
        } else {
            Self::English
        }
    }
}

#[derive(Debug, Clone)]
pub struct Catalog {
    entries: BTreeMap<&'static str, &'static str>,
}

impl Catalog {
    #[must_use]
    pub fn for_language(language: Language) -> Self {
        let resource = match language {
            Language::English => EN_US,
            Language::SimplifiedChinese => ZH_CN,
        };
        Self {
            entries: parse_resource(resource),
        }
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&'static str> {
        self.entries.get(key).copied()
    }
}

fn parse_resource(resource: &'static str) -> BTreeMap<&'static str, &'static str> {
    resource
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            line.split_once('=')
                .map(|(key, value)| (key.trim(), value.trim()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_and_english_have_identical_nonempty_keys() {
        let english = parse_resource(EN_US);
        let chinese = parse_resource(ZH_CN);
        assert_eq!(
            english.keys().collect::<Vec<_>>(),
            chinese.keys().collect::<Vec<_>>()
        );
        assert!(english.values().all(|value| !value.is_empty()));
        assert!(chinese.values().all(|value| !value.is_empty()));
    }

    #[test]
    fn locale_matching_is_conservative() {
        assert_eq!(
            Language::from_locale_tag("zh-CN"),
            Language::SimplifiedChinese
        );
        assert_eq!(Language::from_locale_tag("en-US"), Language::English);
        assert_eq!(Language::from_locale_tag("de-DE"), Language::English);
    }
}
