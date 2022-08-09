pub fn convert_cpu_usage_to_milli(cpu: &str) -> Option<u32> {
    if cpu.is_empty() {
        return None;
    }

    let value: String = cpu.chars().filter(|c| c.is_ascii_digit()).collect();
    let unit: String = cpu.chars().filter(|c| c.is_alphabetic()).collect();

    if value.is_empty() {
        return None;
    }

    let parsed_value: f64 = value.parse().unwrap_or(0f64);
    let mut denominator = 1000000.0;

    if parsed_value < 1.0 || unit.is_empty() {
        return Some((parsed_value * 1000.0).ceil() as u32);
    }

    match unit.as_str() {
        "m" => {
            denominator = 1.0;
        }
        "u" => {
            denominator = 1000.0;
        }
        "n" => {}
        &_ => {
            error!("Unknown CPU unit");
            return None;
        }
    }

    Some((parsed_value / denominator).ceil() as u32)
}
pub fn convert_memory_usage_to_bytes(memory: &str) -> Option<u64> {
    if memory.is_empty() {
        return None;
    }

    let value: String = memory.chars().filter(|c| c.is_ascii_digit()).collect();
    let mut unit: String = memory.chars().filter(|c| c.is_alphabetic()).collect();

    unit = unit.to_lowercase();

    if value.is_empty() {
        return None;
    }

    let parsed_value: u64 = value.parse().unwrap_or(0u64);
    let mut multiplier: u64 = 1024;

    match unit.as_str() {
        "" => {
            multiplier = 1;
        }
        "ki" => {}
        "mi" => {
            multiplier = multiplier.pow(2);
        }
        "gi" => {
            multiplier = multiplier.pow(3);
        }
        "ti" => {
            multiplier = multiplier.pow(4);
        }
        "k" => {
            multiplier = 1000;
        }
        "m" => {
            multiplier = 1000000;
        }
        "g" => {
            multiplier = 1000u64.pow(3);
        }
        &_ => {}
    }

    Some(parsed_value * multiplier)
}

#[cfg(test)]
mod tests {

    use super::*;

    #[tokio::test]
    async fn test_cpu_empty() {
        let result = convert_cpu_usage_to_milli("");

        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_cpu_0() {
        let result = convert_cpu_usage_to_milli("0u");

        assert_eq!(result.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_cpu_unit_empty() {
        let result = convert_cpu_usage_to_milli("100");

        assert_eq!(result.unwrap(), 100000);
    }

    #[tokio::test]
    async fn test_unknown_cpu_unit() {
        let result = convert_cpu_usage_to_milli("100rrr");

        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_cpu_m() {
        let result = convert_cpu_usage_to_milli("100m");

        assert_eq!(result.unwrap(), 100);
    }

    #[tokio::test]
    async fn test_cpu_u() {
        let result = convert_cpu_usage_to_milli("1000u");

        assert_eq!(result.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_less_than_1_converted_cpu_u() {
        let result = convert_cpu_usage_to_milli("10u");

        assert_eq!(result.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_memory_empty() {
        let result = convert_memory_usage_to_bytes("");

        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_memory_empty_but_unit() {
        let result = convert_memory_usage_to_bytes("ki");

        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_memory_no_unit() {
        let result = convert_memory_usage_to_bytes("1000");

        assert_eq!(result.unwrap(), 1000);
    }

    #[tokio::test]
    async fn test_memory_mi() {
        let result = convert_memory_usage_to_bytes("1000mi");

        assert_eq!(result.unwrap(), 1048576000);
    }

    #[tokio::test]
    async fn test_memory_gi() {
        let result = convert_memory_usage_to_bytes("1000gi");

        assert_eq!(result.unwrap(), 1073741824000);
    }

    #[tokio::test]
    async fn test_memory_ti() {
        let result = convert_memory_usage_to_bytes("1000ti");

        assert_eq!(result.unwrap(), 1099511627776000);
    }

    #[tokio::test]
    async fn test_memory_k() {
        let result = convert_memory_usage_to_bytes("1000k");

        assert_eq!(result.unwrap(), 1000000);
    }

    #[tokio::test]
    async fn test_memory_m() {
        let result = convert_memory_usage_to_bytes("1000m");

        assert_eq!(result.unwrap(), 1000000000);
    }

    #[tokio::test]
    async fn test_memory_g() {
        let result = convert_memory_usage_to_bytes("1000g");

        assert_eq!(result.unwrap(), 1000000000000);
    }
}
