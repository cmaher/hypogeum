use async_trait::async_trait;
use hypgm_errors::ServiceError;

#[async_trait]
pub trait HelloWorldService: Send + Sync {
    async fn say_hello(&self, name: &str) -> Result<String, ServiceError>;
}

#[derive(Default)]
pub struct DefaultHelloWorldService;

#[async_trait]
impl HelloWorldService for DefaultHelloWorldService {
    async fn say_hello(&self, name: &str) -> Result<String, ServiceError> {
        let name = name.trim();
        if name.contains('\0') || name.chars().count() > 200 {
            return Err(ServiceError::Validation(
                "Name must contain at most 200 characters without NUL".into(),
            ));
        }
        let name = if name.is_empty() { "world" } else { name };
        Ok(format!("Hello, {name}!"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn greetings_trim_names_and_default_to_world() {
        for (name, expected) in [
            ("", "Hello, world!"),
            (" \n\t\u{2003}", "Hello, world!"),
            ("  Ada  ", "Hello, Ada!"),
            ("\u{2003}世界 🦀\u{2003}", "Hello, 世界 🦀!"),
            ("Ada  Lovelace", "Hello, Ada  Lovelace!"),
        ] {
            assert_eq!(
                DefaultHelloWorldService.say_hello(name).await.unwrap(),
                expected
            );
        }
    }

    #[tokio::test]
    async fn validation_counts_unicode_scalars_after_trimming() {
        let name = "🦀".repeat(200);
        assert_eq!(
            DefaultHelloWorldService
                .say_hello(&format!("  {name}  "))
                .await
                .unwrap(),
            format!("Hello, {name}!")
        );
        for name in ["🦀".repeat(201), "x\0y".into(), "\0".into()] {
            assert!(matches!(
                DefaultHelloWorldService.say_hello(&name).await,
                Err(ServiceError::Validation(_))
            ));
        }
    }
}
