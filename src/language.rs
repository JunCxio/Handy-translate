use anyhow::{Context, Result, ensure};

// 统一语言代码用于模型提示、接口和缓存，不把 auto 写入项目记录。
const CODES: &[(&str, &str)] = &[
    ("jpn", "ja"),
    ("eng", "en"),
    ("cmn", "zh-CN"),
    ("kor", "ko"),
    ("fra", "fr"),
    ("deu", "de"),
    ("spa", "es"),
    ("por", "pt"),
    ("rus", "ru"),
    ("ita", "it"),
    ("vie", "vi"),
    ("tha", "th"),
    ("ind", "id"),
    ("tur", "tr"),
    ("ukr", "uk"),
    ("ara", "ar"),
    ("nld", "nl"),
    ("pol", "pl"),
    ("ces", "cs"),
    ("swe", "sv"),
    ("fin", "fi"),
    ("dan", "da"),
    ("nob", "nb"),
    ("ell", "el"),
    ("heb", "he"),
    ("hin", "hi"),
    ("ron", "ro"),
    ("hun", "hu"),
];

pub fn detect(texts: impl ExactSizeIterator<Item = impl AsRef<str>>) -> Result<String> {
    let step = (texts.len() / 800).max(1);
    let mut sample = String::new();
    for text in texts.step_by(step).take(800) {
        sample.extend(text.as_ref().chars().take(240));
        sample.push('\n');
    }
    ensure!(
        sample.chars().filter(|c| c.is_alphabetic()).count() >= 24,
        "可识别的原文太少，请手动选择源语言后重新加载"
    );
    let info = whatlang::detect(&sample).context("无法可靠识别源语言，请手动选择后重新加载")?;
    ensure!(
        info.is_reliable(),
        "游戏文本包含多种语言或特征不足，请手动选择源语言后重新加载"
    );
    CODES
        .iter()
        .find(|(code, _)| *code == info.lang().code())
        .map(|(_, code)| (*code).into())
        .context("识别出的语言暂不在自动识别范围内，请选择“其他语言”手动填写语言代码")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects_game_text_locally_and_rejects_ambiguous_short_text() {
        for (text, expected) in [
            (
                "今日はいい天気ですね。一緒に町に行きましょう。買い物が終わったら家に戻って休みましょう。",
                "ja",
            ),
            (
                "Welcome to the village. You can buy equipment at the shop, talk to your friends, and explore the forest to find hidden treasure.",
                "en",
            ),
            (
                "오늘은 날씨가 정말 좋습니다. 친구들과 함께 마을에 가서 필요한 물건을 사고 집으로 돌아오겠습니다.",
                "ko",
            ),
            (
                "欢迎来到这个村庄。你可以在商店购买装备，与朋友交谈，还可以探索森林寻找隐藏的宝藏。",
                "zh-CN",
            ),
        ] {
            assert_eq!(detect([text].into_iter()).unwrap(), expected);
        }
        assert!(detect(["OK", "HP"].into_iter()).is_err());
    }
}
