//! 文本处理与杂项（正则提取、随机串、UUID、时间戳换算）。
//!
//! 这些算法单独看都很小，值钱的是组合：`Base64 解码 → 清空白 → 正则提取 → 排序去重`
//! 就是捞 SDK 里硬编码域名的常规四步。所以每一步都必须能独立看懂结果。

use crate::services::chef::data::{Args, ChefData, ChefResult, fail};
use crate::services::chef::spec::{bool_arg, choice, number, op, text as text_arg};

fn upper(data: &ChefData, _args: &Args) -> ChefResult<ChefData> {
    Ok(ChefData::from_text(&data.text()?.to_uppercase()))
}

fn lower(data: &ChefData, _args: &Args) -> ChefResult<ChefData> {
    Ok(ChefData::from_text(&data.text()?.to_lowercase()))
}

fn title_case(data: &ChefData, _args: &Args) -> ChefResult<ChefData> {
    let mut out = String::with_capacity(data.len());
    let mut word_start = true;
    for ch in data.text()?.chars() {
        if ch.is_alphanumeric() {
            out.push(if word_start {
                ch.to_uppercase().next().unwrap_or(ch)
            } else {
                ch.to_lowercase().next().unwrap_or(ch)
            });
            word_start = false;
        } else {
            out.push(ch);
            word_start = true;
        }
    }
    Ok(ChefData::from_text(&out))
}

fn swap_case(data: &ChefData, _args: &Args) -> ChefResult<ChefData> {
    let out: String = data
        .text()?
        .chars()
        .flat_map(|c| {
            if c.is_uppercase() {
                c.to_lowercase().collect::<Vec<_>>()
            } else {
                c.to_uppercase().collect::<Vec<_>>()
            }
        })
        .collect();
    Ok(ChefData::from_text(&out))
}

fn reverse(data: &ChefData, _args: &Args) -> ChefResult<ChefData> {
    Ok(ChefData::from_text(
        &data.text()?.chars().rev().collect::<String>(),
    ))
}

fn trim_whitespace(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let where_ = args.choice_in("where", "both", &["both", "start", "end"])?;
    let source = data.text()?;
    let out = match where_.as_str() {
        "both" => source.trim(),
        "start" => source.trim_start(),
        _ => source.trim_end(),
    };
    Ok(ChefData::from_text(out))
}

fn remove_whitespace(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let where_ = args.choice_in("where", "all", &["all", "start", "end"])?;
    let extra = args.text("characters", "");
    let is_target = |c: char| c.is_whitespace() || extra.chars().any(|e| e == c);
    let source = data.text()?;
    let out: String = match where_.as_str() {
        "all" => source.chars().filter(|c| !is_target(*c)).collect(),
        "start" => {
            let keep = source
                .char_indices()
                .find(|(_, c)| !is_target(*c))
                .map(|(index, _)| index)
                .unwrap_or(source.len());
            source[keep..].to_string()
        }
        _ => {
            let keep = source
                .char_indices()
                .rev()
                .find(|(_, c)| !is_target(*c))
                .map(|(index, c)| index + c.len_utf8())
                .unwrap_or(0);
            source[..keep].to_string()
        }
    };
    Ok(ChefData::from_text(&out))
}

fn clean_whitespace(data: &ChefData, _args: &Args) -> ChefResult<ChefData> {
    Ok(ChefData::from_text(
        &data
            .text()?
            .split_whitespace()
            .collect::<Vec<&str>>()
            .join(" "),
    ))
}

fn replace(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let search = args.text("search", "");
    if search.is_empty() {
        return Err(fail("还差一步：要替换的内容（search）是空的"));
    }
    let replacement = args.text("replacement", "");
    let source = data.text()?;
    let out = if args.bool_of("useRegex") {
        let re = regex::Regex::new(&search).map_err(|e| fail(format!("正则不合法：{e}")))?;
        if args.bool_of("global") {
            re.replace_all(source, replacement.as_str()).into_owned()
        } else {
            re.replace(source, replacement.as_str()).into_owned()
        }
    } else if args.bool_of("global") {
        source.replace(search.as_str(), replacement.as_str())
    } else {
        source.replacen(search.as_str(), replacement.as_str(), 1)
    };
    Ok(ChefData::from_text(&out))
}

fn regex_extract(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let pattern = args.text("pattern", "");
    if pattern.is_empty() {
        return Err(fail("还差一步：正则（pattern）是空的"));
    }
    let re = regex::Regex::new(&pattern).map_err(|e| fail(format!("正则不合法：{e}")))?;
    let group = args.number("group", 0)? as usize;
    let source = data.text()?;
    let mut hits: Vec<String> = Vec::new();
    for caps in re.captures_iter(source) {
        let matched = caps.get(group).ok_or_else(|| {
            fail(format!(
                "正则里没有第 {group} 组；写成 (…) 才有第 1 组，整体匹配是第 0 组"
            ))
        })?;
        hits.push(matched.as_str().to_string());
    }
    if hits.is_empty() {
        return Err(fail("一处也没匹配到"));
    }
    Ok(ChefData::from_text(&hits.join("\n")))
}

fn count_occurrences(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let pattern = args.text("pattern", "");
    if pattern.is_empty() {
        return Err(fail("还差一步：要统计的内容（pattern）是空的"));
    }
    let source = data.text()?;
    let total = if args.bool_of("useRegex") {
        let re = regex::Regex::new(&pattern).map_err(|e| fail(format!("正则不合法：{e}")))?;
        re.find_iter(source).count()
    } else {
        source.matches(pattern.as_str()).count()
    };
    Ok(ChefData::from_text(&total.to_string()))
}

/// 分隔符参数里允许写 `\n`（界面上敲不进真空格）
fn unescape_separator(raw: &str) -> String {
    raw.replace("\\n", "\n").replace("\\t", "\t")
}

fn sort_lines(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let order = args.choice_in("order", "asc", &["asc", "desc"])?;
    let by = args.choice_in("by", "text", &["text", "number", "length"])?;
    let separator = unescape_separator(args.text("separator", "\n").as_str());
    let mut lines: Vec<String> = data
        .text()?
        .split(separator.as_str())
        .filter(|line| !(args.bool_of("skipEmpty") && line.is_empty()))
        .map(str::to_string)
        .collect();
    lines.sort_by(|a, b| match by.as_str() {
        "number" => {
            let left = a.trim().parse::<f64>().unwrap_or(f64::NAN);
            let right = b.trim().parse::<f64>().unwrap_or(f64::NAN);
            left.partial_cmp(&right)
                .unwrap_or(core::cmp::Ordering::Equal)
        }
        "length" => a.chars().count().cmp(&b.chars().count()),
        _ => a.cmp(b),
    });
    if order == "desc" {
        lines.reverse();
    }
    if args.bool_of("unique") {
        lines.dedup();
    }
    Ok(ChefData::from_text(&lines.join(&separator)))
}

fn dedupe_lines(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let source = data.text()?;
    let mut out: Vec<&str> = Vec::new();
    if args.bool_of("keepOrder") {
        for line in source.lines() {
            if !out.contains(&line) {
                out.push(line);
            }
        }
    } else {
        let mut lines: Vec<&str> = source.lines().collect();
        lines.sort_unstable();
        lines.dedup();
        out = lines;
    }
    Ok(ChefData::from_text(&out.join("\n")))
}

fn word_frequency(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let top = args.number("top", 20)?.max(1) as usize;
    let ignore_case = args.bool_of("ignoreCase");
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    for word in data.text()?.split_whitespace() {
        let key = if ignore_case {
            word.to_lowercase()
        } else {
            word.to_string()
        };
        *counts.entry(key).or_insert(0) += 1;
    }
    let mut ranked: Vec<(String, usize)> = counts.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let ranked = ranked;
    let out: Vec<String> = ranked
        .into_iter()
        .take(top)
        .map(|(word, count)| format!("{}\t{}", count, word))
        .collect();
    if out.is_empty() {
        return Err(fail("一个词也没有"));
    }
    Ok(ChefData::from_text(&out.join("\n")))
}

fn text_stats(data: &ChefData, _args: &Args) -> ChefResult<ChefData> {
    let source = data.text()?;
    let non_ascii = source.chars().filter(|c| !c.is_ascii()).count();
    Ok(ChefData::from_text(&format!(
        "字符\t{chars}\n字节\t{bytes}\n行\t{lines}\n词\t{words}\n非 ASCII 字符\t{non_ascii}",
        chars = source.chars().count(),
        bytes = data.bytes.len(),
        lines = if source.is_empty() {
            0
        } else {
            source.lines().count()
        },
        words = source.split_whitespace().count(),
        non_ascii = non_ascii,
    )))
}

fn truncate_string(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let length = args.number("length", 100)? as usize;
    let from = args.choice_in("from", "start", &["start", "end"])?;
    let chars: Vec<char> = data.text()?.chars().collect();
    let out: String = if from == "end" {
        chars
            .iter()
            .rev()
            .take(length)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    } else {
        chars.iter().take(length).collect()
    };
    Ok(ChefData::from_text(&out))
}

fn slice_bytes(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    // `length = 0` 表示「一直到末尾」；`from = end` 取尾巴若干字节。
    // 脱壳里两种切法就够用了：去掉固定头部、只留末尾的对齐块。
    let total = data.bytes.len();
    let length = args.number("length", 0)? as usize;
    let from = args.choice_in("from", "start", &["start", "end"])?;
    let range = match from.as_str() {
        "start" => {
            let start = args.number("start", 0)? as usize;
            if start >= total {
                return Err(fail(format!("start {start} 已经超出数据长度 {total} 字节")));
            }
            let end = if length == 0 {
                total
            } else {
                (start + length).min(total)
            };
            start..end
        }
        _ => {
            if length == 0 {
                return Err(fail("从尾部取就要给 length（0 在这里没有意义）"));
            }
            let take = length.min(total);
            (total - take)..total
        }
    };
    if range.is_empty() {
        return Err(fail("切片范围为空，什么都没取到"));
    }
    Ok(ChefData::new(data.bytes[range].to_vec()))
}

fn split_on_separator(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let separator = unescape_separator(&args.text("separator", ","));
    let parts = data.text()?.split(separator.as_str());
    let kept: Vec<&str> = if args.bool_of("skipEmpty") {
        parts.filter(|part| !part.is_empty()).collect()
    } else {
        parts.collect()
    };
    Ok(ChefData::from_text(&kept.join("\n")))
}

fn join_lines(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let separator = unescape_separator(&args.text("separator", ","));
    Ok(ChefData::from_text(
        &data.text()?.lines().collect::<Vec<&str>>().join(&separator),
    ))
}

fn repeat_string(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let count = args.number("count", 3)?;
    if count > 10_000_000 {
        return Err(fail("重复次数上限 1000 万，再大就只是把内存吃掉"));
    }
    let source = data.text()?.to_string();
    // 分配之前先算总长度：单步上限（挡解压炸弹的那道闸）对「重复」同样适用
    let planned = source.len().saturating_mul(count as usize);
    if planned > crate::services::chef::MAX_STEP_BYTES {
        return Err(fail(format!(
            "重复后会产出 {planned} 字节，超过单步上限 {} 字节（这条闸门同时也是压缩炸弹的防线）",
            crate::services::chef::MAX_STEP_BYTES
        )));
    }
    let mut out = String::with_capacity(planned);
    for _ in 0..count {
        out.push_str(&source);
    }
    Ok(ChefData::from_text(&out))
}

/// 提取常用模式：逆向的第一刀通常从这里下
const EXTRACT_KINDS: &[&str] = &[
    "url", "email", "ipv4", "mac", "package", "uuid", "imei", "phone-cn", "hex32", "base64", "jwt",
    "log-tag",
];

fn extract(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let kind = args.choice_in("kind", "url", EXTRACT_KINDS)?;
    let pattern = match kind.as_str() {
        "url" => r#"https?://[^\s'"]+"#,
        "email" => r"[\w.+-]+@[\w-]+\.[\w.-]+",
        "ipv4" => r"\b(?:\d{1,3}\.){3}\d{1,3}\b",
        "mac" => r"(?:[0-9A-Fa-f]{2}:){5}[0-9A-Fa-f]{2}",
        "package" => r"\b(?:[A-Za-z_]\w*\.)+[A-Za-z_]\w*\b",
        "uuid" => r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}",
        "imei" => r"\b\d{14,16}\b",
        "phone-cn" => r"\b1[3-9]\d{9}\b",
        "hex32" => r"\b[0-9a-fA-F]{32}\b",
        "base64" => r"\b[A-Za-z0-9+/]{16,}={0,2}\b",
        "jwt" => r"eyJ[\w-]{5,}\.[\w-]{5,}\.[\w-]{5,}",
        "log-tag" => r"\b[VDIWEAF]/[\w.$/]+(?:\(\d+\))?\s*:",
        other => return Err(fail(format!("未知的提取类型「{other}」"))),
    };
    let re = regex::Regex::new(pattern).expect("内置正则一定合法");
    let mut hits: Vec<String> = re
        .find_iter(data.text()?)
        .map(|m| m.as_str().to_string())
        .collect();
    let empty = hits.is_empty();
    if args.bool_of("unique") {
        hits.sort();
        hits.dedup();
    }
    if empty {
        return Err(fail(format!("按「{kind}」一处也没提取到")));
    }
    Ok(ChefData::from_text(&hits.join("\n")))
}

// ---------------------------------------------------------------------------
// 时间戳（公历换算自己写：不引 chrono，界面上只需要 ISO 与 Unix 两种口径）
// ---------------------------------------------------------------------------

/// Howard Hinnant 的 days_from_civil / civil_from_days：跨 1970 与闰年都不出错
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = if month > 2 { month - 3 } else { month + 9 } as u64;
    let doy = (153 * mp + 2) / 5 + day as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { y + 1 } else { y }, month, day)
}

/// Unix 秒 + 时区偏移（分钟）→ ISO 8601。JWT、日志、证书里到处要用。
pub fn format_unix(seconds: i64, offset_minutes: i64) -> Option<String> {
    let local = seconds.checked_add(offset_minutes.checked_mul(60)?)?;
    let days = local.div_euclid(86_400);
    let second_of_day = local.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    if !(0..=9999).contains(&year) {
        return None;
    }
    let zone = if offset_minutes == 0 {
        "Z".to_string()
    } else {
        let sign = if offset_minutes < 0 { '-' } else { '+' };
        let absolute = offset_minutes.abs();
        format!("{sign}{:02}:{:02}", absolute / 60, absolute % 60)
    };
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}{zone}",
        second_of_day / 3600,
        (second_of_day % 3600) / 60,
        second_of_day % 60,
    ))
}

const TIME_UNITS: &[&str] = &["seconds", "milliseconds", "microseconds", "nanoseconds"];
const DATE_TIME_UNITS: &[&str] = &["seconds", "milliseconds"];

fn divisor_of(unit: &str) -> ChefResult<i128> {
    Ok(match unit {
        "seconds" => 1,
        "milliseconds" => 1_000,
        "microseconds" => 1_000_000,
        "nanoseconds" => 1_000_000_000,
        other => return Err(fail(format!("未知的时间单位「{other}」"))),
    })
}

fn unix_to_date(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let unit = args.choice_in("unit", "milliseconds", TIME_UNITS)?;
    let divisor = divisor_of(&unit)?;
    let raw = data.text()?.trim().replace('_', "");
    let number: i128 = raw.parse().map_err(|_| {
        fail(format!(
            "「{raw}」不是整数时间戳（十进制；毫秒级 13 位、秒级 10 位）"
        ))
    })?;
    let seconds = (number / divisor) as i64;
    let offset = args.number("offsetMinutes", 0)? as i64;
    let local = format_unix(seconds, offset)
        .ok_or_else(|| fail("换算结果落在公元 0–9999 之外：先确认单位没选错"))?;
    let utc = format_unix(seconds, 0).unwrap_or_default();
    Ok(ChefData::from_text(&format!(
        "本地 {local}\nUTC   {utc}\n秒    {seconds}\n毫秒  {}",
        seconds as i128 * 1_000
    )))
}

fn date_to_unix(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let source = data.text()?.trim().replace('T', " ").replace('/', "-");
    let Some((date_part, rest)) = source.split_once(' ') else {
        return Err(fail(
            "时间要写成 2026-09-30 12:34:56Z 或 2026-09-30T12:34:56+08:00（日期与时间之间要有分隔）",
        ));
    };
    let parts: Vec<&str> = date_part.split('-').collect();
    if parts.len() != 3 {
        return Err(fail(format!("日期部分「{date_part}」不是一段 年-月-日")));
    }
    let number = |text: &str, what: &str| -> ChefResult<i64> {
        text.parse::<i64>()
            .map_err(|_| fail(format!("{what}「{text}」不是数字")))
    };
    let year = number(parts[0], "年")?;
    let month = number(parts[1], "月")? as u32;
    let day = number(parts[2], "日")? as u32;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return Err(fail(format!("月/日超出范围：{month}-{day}")));
    }
    // 尾巴上的 Z / +08:00 / -0500 优先；没写偏移才用 offsetMinutes
    let zone_index = rest.find(['+', '-']).filter(|index| *index > 0);
    let (clock, offset) = match zone_index {
        Some(index) => {
            let sign = if rest.as_bytes()[index] == b'-' {
                -1
            } else {
                1
            };
            let digits: String = rest[index + 1..]
                .chars()
                .filter(|c| c.is_ascii_digit())
                .collect();
            let hour = digits.get(..2).unwrap_or("00").parse::<i64>().unwrap_or(0);
            let minute = digits.get(2..4).unwrap_or("00").parse::<i64>().unwrap_or(0);
            (rest[..index].to_string(), sign * (hour * 60 + minute))
        }
        None => (
            rest.trim_end_matches('Z').to_string(),
            args.number("offsetMinutes", 0)? as i64,
        ),
    };
    let clock_parts: Vec<&str> = clock.split(':').collect();
    let at = |index: usize| {
        clock_parts
            .get(index)
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(0)
    };
    let (hour, minute, second) = (at(0), at(1), at(2));
    if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) || !(0..=60).contains(&second) {
        return Err(fail(format!(
            "时间部分「{clock}」超出范围（时 0–23，分秒 0–59）"
        )));
    }
    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second
        - offset * 60;
    let unit = args.choice_in("unit", "seconds", DATE_TIME_UNITS)?;
    let value = if unit == "milliseconds" {
        seconds * 1_000
    } else {
        seconds
    };
    Ok(ChefData::from_text(&value.to_string()))
}

fn generate_uuid(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let _ = data;
    let count = args.number("count", 1)?.clamp(1, 1000) as usize;
    let mut lines = Vec::with_capacity(count);
    for _ in 0..count {
        let mut text = uuid::Uuid::new_v4().to_string();
        if args.bool_of("noHyphens") {
            text = text.replace('-', "");
        }
        if args.bool_of("uppercase") {
            text = text.to_uppercase();
        }
        lines.push(text);
    }
    Ok(ChefData::from_text(&lines.join("\n")))
}

const POOL_ALNUM: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
const POOL_ALPHA: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
const POOL_DIGIT: &[u8] = b"0123456789";
const POOL_HEX: &[u8] = b"0123456789abcdef";

fn random_string(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let _ = data;
    use rand::Rng;
    let length = args.number("length", 16)?.clamp(1, 4096) as usize;
    let charset = args.choice_in(
        "charset",
        "alnum",
        &["alnum", "alpha", "digit", "hex", "custom"],
    )?;
    let pool: Vec<u8> = match charset.as_str() {
        "alnum" => POOL_ALNUM.to_vec(),
        "alpha" => POOL_ALPHA.to_vec(),
        "digit" => POOL_DIGIT.to_vec(),
        "hex" => POOL_HEX.to_vec(),
        "custom" => {
            let custom = args.text("characters", "");
            if custom.is_empty() {
                return Err(fail("选了 custom 就得把 characters 填上"));
            }
            custom.into_bytes()
        }
        other => return Err(fail(format!("未知字符集「{other}」"))),
    };
    let mut rng = rand::thread_rng();
    let out: String = (0..length)
        .map(|_| pool[rng.gen_range(0..pool.len())] as char)
        .collect();
    Ok(ChefData::from_text(&out))
}

pub const OPS: &[super::Op] = &[
    op("toUpperCase", "text", &[], upper),
    op("toLowerCase", "text", &[], lower),
    op("toTitleCase", "text", &[], title_case),
    op("swapCase", "text", &[], swap_case),
    op("reverseString", "text", &[], reverse),
    op(
        "trimWhitespace",
        "text",
        &[choice("where", "both", &["both", "start", "end"])],
        trim_whitespace,
    ),
    op(
        "removeWhitespace",
        "text",
        &[
            choice("where", "all", &["all", "start", "end"]),
            text_arg("characters", ""),
        ],
        remove_whitespace,
    ),
    op("cleanWhitespace", "text", &[], clean_whitespace),
    op(
        "replace",
        "text",
        &[
            text_arg("search", ""),
            text_arg("replacement", ""),
            bool_arg("useRegex", false),
            bool_arg("global", true),
        ],
        replace,
    ),
    op(
        "regexExtract",
        "text",
        &[text_arg("pattern", ""), number("group", 0)],
        regex_extract,
    ),
    op(
        "countOccurrences",
        "text",
        &[text_arg("pattern", ""), bool_arg("useRegex", false)],
        count_occurrences,
    ),
    op(
        "sortLines",
        "text",
        &[
            choice("order", "asc", &["asc", "desc"]),
            choice("by", "text", &["text", "number", "length"]),
            text_arg("separator", "\n"),
            bool_arg("unique", false),
            bool_arg("skipEmpty", true),
        ],
        sort_lines,
    ),
    op(
        "dedupeLines",
        "text",
        &[bool_arg("keepOrder", true)],
        dedupe_lines,
    ),
    op(
        "wordFrequency",
        "text",
        &[number("top", 20), bool_arg("ignoreCase", true)],
        word_frequency,
    ),
    op("textStats", "text", &[], text_stats),
    op(
        "truncateString",
        "text",
        &[
            number("length", 100),
            choice("from", "start", &["start", "end"]),
        ],
        truncate_string,
    ),
    op(
        "sliceBytes",
        "text",
        &[
            number("start", 0),
            number("length", 0),
            choice("from", "start", &["start", "end"]),
        ],
        slice_bytes,
    ),
    op(
        "splitOnSeparator",
        "text",
        &[text_arg("separator", ","), bool_arg("skipEmpty", false)],
        split_on_separator,
    ),
    op(
        "joinLines",
        "text",
        &[text_arg("separator", ",")],
        join_lines,
    ),
    op("repeatString", "text", &[number("count", 3)], repeat_string),
    op(
        "extract",
        "text",
        &[
            choice("kind", "url", EXTRACT_KINDS),
            bool_arg("unique", true),
        ],
        extract,
    ),
    op(
        "unixToDate",
        "misc",
        &[
            choice("unit", "milliseconds", TIME_UNITS),
            number("offsetMinutes", 0),
        ],
        unix_to_date,
    ),
    op(
        "dateToUnix",
        "misc",
        &[
            choice("unit", "seconds", DATE_TIME_UNITS),
            number("offsetMinutes", 0),
        ],
        date_to_unix,
    ),
    op(
        "generateUuid",
        "misc",
        &[
            number("count", 1),
            bool_arg("uppercase", false),
            bool_arg("noHyphens", false),
        ],
        generate_uuid,
    ),
    op(
        "randomString",
        "misc",
        &[
            number("length", 16),
            choice(
                "charset",
                "alnum",
                &["alnum", "alpha", "digit", "hex", "custom"],
            ),
            text_arg("characters", ""),
        ],
        random_string,
    ),
];

#[cfg(test)]
mod tests {
    use crate::services::chef::testing::{expect_error, run_op_bytes, run_op_text};

    #[test]
    fn case_and_whitespace() {
        assert_eq!(run_op_text("toUpperCase", "abc 中", &[]), "ABC 中");
        assert_eq!(run_op_text("toLowerCase", "ABC 中", &[]), "abc 中");
        assert_eq!(
            run_op_text("toTitleCase", "hello world task", &[]),
            "Hello World Task"
        );
        assert_eq!(run_op_text("swapCase", "aB cD", &[]), "Ab Cd");
        assert_eq!(run_op_text("reverseString", "abcd", &[]), "dcba");
        // 中文按字符反转，不是按字节：按字节会出替换符
        assert_eq!(run_op_text("reverseString", "算法中", &[]), "中法算");
        assert_eq!(run_op_text("trimWhitespace", "  hi  ", &[]), "hi");
        assert_eq!(
            run_op_text("trimWhitespace", "  hi  ", &[("where", "start")]),
            "hi  "
        );
        assert_eq!(run_op_text("removeWhitespace", " a b\tc", &[]), "abc");
        assert_eq!(
            run_op_text("removeWhitespace", "a-_b", &[("characters", "-_")]),
            "ab"
        );
        assert_eq!(
            run_op_text("removeWhitespace", "  a  b  ", &[("where", "start")]),
            "a  b  "
        );
        assert_eq!(
            run_op_text("cleanWhitespace", "  a \n\t b   c ", &[]),
            "a b c"
        );
    }

    #[test]
    fn replace_and_regex() {
        assert_eq!(
            run_op_text("replace", "aaa", &[("search", "a"), ("replacement", "b")]),
            "bbb"
        );
        assert_eq!(
            run_op_text(
                "replace",
                "aaa",
                &[("search", "a"), ("replacement", "b"), ("global", "false")]
            ),
            "baa"
        );
        assert_eq!(
            run_op_text(
                "replace",
                "id=42 name=x",
                &[
                    ("search", r"\d+"),
                    ("replacement", "N"),
                    ("useRegex", "true")
                ]
            ),
            "id=N name=x"
        );
        assert_eq!(
            run_op_text(
                "regexExtract",
                "a1b22c333",
                &[("pattern", r"\d+"), ("group", "0")]
            ),
            "1\n22\n333"
        );
        // 取捕获组：这是逆向里最常见的一次调用
        assert_eq!(
            run_op_text(
                "regexExtract",
                "key=abc&token=def",
                &[("pattern", r"=(\w+)"), ("group", "1")]
            ),
            "abc\ndef"
        );
        let message = expect_error("regexExtract", "abc", &[("pattern", r"zz"), ("group", "0")]);
        assert!(message.contains("一处也没匹配到"), "{message}");
        let message = expect_error(
            "regexExtract",
            "abc",
            &[("pattern", r"(a)(b)"), ("group", "5")],
        );
        assert!(message.contains("第 5 组"), "{message}");
        assert_eq!(
            run_op_text("countOccurrences", "a-b-c", &[("pattern", "-")]),
            "2"
        );
        assert_eq!(
            run_op_text(
                "countOccurrences",
                "aa1bb2",
                &[("pattern", r"\d"), ("useRegex", "true")]
            ),
            "2"
        );
        assert!(expect_error("replace", "abc", &[("search", "")]).contains("还差一步"));
        assert!(
            expect_error("replace", "abc", &[("search", "("), ("useRegex", "true")])
                .contains("正则不合法")
        );
    }

    #[test]
    fn lines_and_stats() {
        assert_eq!(run_op_text("sortLines", "c\na\nb", &[]), "a\nb\nc");
        assert_eq!(
            run_op_text("sortLines", "a\nb\nc", &[("order", "desc")]),
            "c\nb\na"
        );
        assert_eq!(
            run_op_text("sortLines", "10\n9\n100", &[("by", "number")]),
            "9\n10\n100"
        );
        assert_eq!(
            run_op_text("sortLines", "aaa\nb\ncc", &[("by", "length")]),
            "b\ncc\naaa"
        );
        assert_eq!(
            run_op_text("sortLines", "b\nb\na", &[("unique", "true")]),
            "a\nb"
        );
        // 默认保序（捞字符串时顺序有意义），要排序去重就显式关掉
        assert_eq!(run_op_text("dedupeLines", "b\na\nb", &[]), "b\na");
        assert_eq!(
            run_op_text("dedupeLines", "b\na\nb\na", &[("keepOrder", "false")]),
            "a\nb"
        );
        // 默认忽略大小写：B 并入 b；同频次按字典序排，结果才可复现
        assert_eq!(
            run_op_text("wordFrequency", "a a b B", &[("top", "2")]),
            "2\ta\n2\tb"
        );
        assert_eq!(
            run_op_text(
                "wordFrequency",
                "a a b B",
                &[("top", "2"), ("ignoreCase", "false")]
            ),
            "2\ta\n1\tB"
        );
        let stats = run_op_text("textStats", "ab cd\nef", &[]);
        assert!(stats.contains("字符\t8"), "{stats}");
        assert!(stats.contains("行\t2"), "{stats}");
        assert!(stats.contains("词\t3"), "{stats}");
        assert_eq!(
            run_op_text("truncateString", "abcdef", &[("length", "3")]),
            "abc"
        );
        assert_eq!(
            run_op_text(
                "truncateString",
                "abcdef",
                &[("length", "3"), ("from", "end")]
            ),
            "def"
        );
        assert_eq!(
            run_op_text("splitOnSeparator", "a,,b", &[("separator", ",")]),
            "a\n\nb"
        );
        assert_eq!(
            run_op_text(
                "splitOnSeparator",
                "a,,b",
                &[("separator", ","), ("skipEmpty", "true")]
            ),
            "a\nb"
        );
        assert_eq!(
            run_op_text("joinLines", "a\nb", &[("separator", ",")]),
            "a,b"
        );
        assert_eq!(
            run_op_text("repeatString", "ab", &[("count", "3")]),
            "ababab"
        );
    }

    #[test]
    fn byte_slicing() {
        // 「去掉头 4 字节」与「只留末尾 16 字节」是脱壳里两种固定切法
        let data: Vec<u8> = (0..32u8).collect();
        let head_less = run_op_bytes("sliceBytes", &data, &[("start", "4")]).bytes;
        assert_eq!(head_less.len(), 28);
        assert_eq!(head_less[0], 4);
        let tail = run_op_bytes("sliceBytes", &data, &[("length", "16"), ("from", "end")]).bytes;
        assert_eq!(tail.len(), 16);
        assert_eq!(tail[0], 16);
        let whole = run_op_bytes("sliceBytes", &data, &[]).bytes;
        assert_eq!(whole, data);
        assert!(
            expect_error(
                "sliceBytes",
                &String::from_utf8_lossy(&data),
                &[("start", "99")]
            )
            .contains("超出数据长度")
        );
    }

    #[test]
    fn extract_presets() {
        let sample = "见 https://api.example.com/v1/sign?k=1 与 user@example.com ，\
                       设备 192.168.1.23，网卡 AA:BB:CC:DD:EE:01，包名 com.tools.demo，\
                       手机 13800138000，IMEI 861234567890123，uid 900150983cd24fb0d6963f7d28e17f72";
        for (kind, expected) in [
            ("url", "https://api.example.com/v1/sign?k=1"),
            ("email", "user@example.com"),
            ("ipv4", "192.168.1.23"),
            ("mac", "AA:BB:CC:DD:EE:01"),
            ("phone-cn", "13800138000"),
            ("hex32", "900150983cd24fb0d6963f7d28e17f72"),
            ("imei", "861234567890123"),
        ] {
            let out = run_op_text("extract", sample, &[("kind", kind)]);
            assert!(out.contains(expected), "类型 {kind} 得到：{out}");
        }
        let packages = run_op_text("extract", sample, &[("kind", "package")]);
        assert!(packages.contains("com.tools.demo"), "{packages}");
        let message = expect_error("extract", "啥也没有", &[("kind", "email")]);
        assert!(message.contains("一处也没提取到"), "{message}");
    }

    #[test]
    fn timestamps_round_trip() {
        // 2001-09-09T01:46:40Z = 1000000000 秒（RFC 3143 那个著名时刻）
        assert_eq!(
            run_op_text("unixToDate", "1000000000", &[("unit", "seconds")])
                .lines()
                .nth(1)
                .unwrap_or_default()
                .trim(),
            "UTC   2001-09-09T01:46:40Z"
        );
        // 毫秒级：JWT 的 exp 常常给毫秒
        let out = run_op_text("unixToDate", "1767225600000", &[]);
        assert!(out.contains("2026-01-01T00:00:00Z"), "{out}");
        // +08:0 的本地时间要提前 8 小时
        let local = run_op_text("unixToDate", "1767225600000", &[("offsetMinutes", "480")]);
        assert!(local.contains("2026-01-01T08:00:00+08:00"), "{local}");
        // 反方向
        for (text, expected) in [
            ("2001-09-09 01:46:40Z", "1000000000"),
            ("2026-01-01T08:00:00+08:00", "1767225600"),
            ("2026-01-01 00:00:00", "1767225600"),
        ] {
            assert_eq!(
                run_op_text("dateToUnix", text, &[]),
                expected,
                "解析 {text}"
            );
        }
        assert_eq!(
            run_op_text(
                "dateToUnix",
                "2026-01-01 00:00:00",
                &[("unit", "milliseconds")]
            ),
            "1767225600000"
        );
        // 时间戳 → 日期 → 时间戳 必须回到原点
        let stamp = run_op_text("dateToUnix", "1999-12-31 23:59:59Z", &[]);
        let back = run_op_text("unixToDate", &stamp, &[("unit", "seconds")]);
        assert!(back.contains("1999-12-31T23:59:59Z"), "{back}");
        assert!(expect_error("dateToUnix", "20260101", &[]).contains("时间要写成"));
        assert!(expect_error("dateToUnix", "2026-13-01 00:00:00", &[]).contains("超出范围"));
        assert!(expect_error("unixToDate", "abc", &[]).contains("不是整数时间戳"));
    }

    #[test]
    fn uuid_and_random_string() {
        let one = run_op_text("generateUuid", "", &[]);
        assert_eq!(one.chars().count(), 36, "{one}");
        assert_eq!(one.split('-').count(), 5, "{one}");
        assert_eq!(
            run_op_text("generateUuid", "", &[("count", "5")])
                .lines()
                .count(),
            5
        );
        let tight = run_op_text(
            "generateUuid",
            "",
            &[("noHyphens", "true"), ("uppercase", "true")],
        );
        assert_eq!(tight.chars().count(), 32);
        assert!(tight.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
        // 随机串：长度与字符集都要守住
        let hex = run_op_text("randomString", "", &[("length", "40"), ("charset", "hex")]);
        assert_eq!(hex.chars().count(), 40);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()), "{hex}");
        let digits = run_op_text("randomString", "", &[("length", "8"), ("charset", "digit")]);
        assert!(digits.chars().all(|c| c.is_ascii_digit()), "{digits}");
        assert!(
            run_op_text(
                "randomString",
                "",
                &[
                    ("length", "6"),
                    ("charset", "custom"),
                    ("characters", "abc")
                ]
            )
            .chars()
            .all(|c| "abc".contains(c))
        );
        // 两次结果不该一样（随机性坏了就是这里）
        assert_ne!(
            run_op_text("randomString", "", &[("length", "32")]),
            run_op_text("randomString", "", &[("length", "32")])
        );
        assert!(expect_error("randomString", "", &[("charset", "custom")]).contains("characters"));
    }
}
