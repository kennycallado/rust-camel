use camel_api::{Body, Exchange, Message};
use camel_bean::{BeanProcessor, BeanRegistry, bean_impl, handler};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct Order {
    id: String,
    amount: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct ProcessedOrder {
    order_id: String,
    status: String,
}

struct OrderService;

#[bean_impl]
impl OrderService {
    #[handler]
    pub async fn process(&self, body: Order) -> Result<ProcessedOrder, String> {
        Ok(ProcessedOrder {
            order_id: body.id,
            status: "processed".to_string(),
        })
    }

    #[handler]
    pub async fn validate(&self, body: Order) -> Result<bool, String> {
        Ok(body.amount > 0)
    }

    // Non-handler method - should be ignored
    #[allow(dead_code)]
    pub fn helper(&self) -> String {
        "helper".to_string()
    }
}

#[tokio::test]
async fn test_bean_registration() {
    let mut registry = BeanRegistry::new();
    registry.register("orderService", OrderService);

    assert!(registry.get("orderService").is_some());
}

#[tokio::test]
async fn test_bean_methods() {
    let service = OrderService;
    let methods = service.methods();

    assert!(methods.contains(&"process"));
    assert!(methods.contains(&"validate"));
    assert!(!methods.contains(&"helper"));
}

#[tokio::test]
async fn test_invoke_process_method() {
    let mut registry = BeanRegistry::new();
    registry.register("orderService", OrderService);

    let order = Order {
        id: "123".to_string(),
        amount: 100,
    };

    let message = Message {
        body: Body::Json(serde_json::to_value(&order).unwrap()),
        ..Default::default()
    };

    let mut exchange = Exchange::new(message);
    registry
        .invoke("orderService", "process", &mut exchange)
        .await
        .unwrap();

    // Verify body was updated
    match &exchange.input.body {
        Body::Json(value) => {
            let processed: ProcessedOrder = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(processed.order_id, "123");
            assert_eq!(processed.status, "processed");
        }
        _ => panic!("Expected Json body"),
    }
}

#[tokio::test]
async fn test_invoke_validate_method() {
    let mut registry = BeanRegistry::new();
    registry.register("orderService", OrderService);

    let order = Order {
        id: "456".to_string(),
        amount: 50,
    };

    let message = Message {
        body: Body::Json(serde_json::to_value(&order).unwrap()),
        ..Default::default()
    };

    let mut exchange = Exchange::new(message);
    registry
        .invoke("orderService", "validate", &mut exchange)
        .await
        .unwrap();

    // Verify body was updated
    match &exchange.input.body {
        Body::Json(value) => {
            let valid: bool = serde_json::from_value(value.clone()).unwrap();
            assert!(valid);
        }
        _ => panic!("Expected Json body"),
    }
}

#[tokio::test]
async fn test_invoke_unknown_method() {
    let mut registry = BeanRegistry::new();
    registry.register("orderService", OrderService);

    let order = Order {
        id: "789".to_string(),
        amount: 75,
    };

    let message = Message {
        body: Body::Json(serde_json::to_value(&order).unwrap()),
        ..Default::default()
    };

    let mut exchange = Exchange::new(message);
    let result = registry
        .invoke("orderService", "unknown", &mut exchange)
        .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_invoke_unknown_bean() {
    let registry = BeanRegistry::new();
    let mut exchange = Exchange::new(Message::default());

    let result = registry
        .invoke("unknownService", "process", &mut exchange)
        .await;

    assert!(result.is_err());
}
