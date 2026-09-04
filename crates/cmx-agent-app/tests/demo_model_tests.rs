//! DemoModel 关键词路由测试：不同用户输入 → 正确工具调用；有工具结果 → 收尾。

use cmx_agent_app::DemoModel;
use cmx_agent_core::model::{ModelContext, ModelMessage, ModelSeam};

fn ctx_with_user(text: &str) -> ModelContext {
    ModelContext {
        system: None,
        messages: vec![ModelMessage::User { text: text.into() }],
        tools: vec![],
    }
}

#[tokio::test]
async fn routes_flow_keyword_to_flow_tool() {
    let resp = DemoModel
        .complete(&ctx_with_user("帮我列出所有流程定义"))
        .await
        .unwrap();
    assert_eq!(resp.tool_calls.len(), 1);
    assert_eq!(resp.tool_calls[0].name, "flow_list_definitions");
}

#[tokio::test]
async fn routes_object_keyword_to_onto_tool() {
    let resp = DemoModel
        .complete(&ctx_with_user("有哪些对象类型"))
        .await
        .unwrap();
    assert_eq!(resp.tool_calls[0].name, "onto_list_object_types");
}

#[tokio::test]
async fn routes_add_keyword_and_extracts_numbers() {
    let resp = DemoModel
        .complete(&ctx_with_user("帮我算 40 加 2"))
        .await
        .unwrap();
    assert_eq!(resp.tool_calls[0].name, "add");
    assert_eq!(resp.tool_calls[0].input["a"], 40.0);
    assert_eq!(resp.tool_calls[0].input["b"], 2.0);
}

#[tokio::test]
async fn unknown_input_yields_text_no_tool() {
    let resp = DemoModel.complete(&ctx_with_user("你好呀")).await.unwrap();
    assert!(resp.tool_calls.is_empty());
    assert!(resp.text.is_some());
}

#[tokio::test]
async fn summarizes_flow_result_when_tool_output_present() {
    // 上下文里已有一个 flow 工具结果 → 模型应收尾成一句自然语言，不再调工具
    let ctx = ModelContext {
        system: None,
        messages: vec![
            ModelMessage::User {
                text: "列出流程".into(),
            },
            ModelMessage::Assistant {
                text: Some("我去问……".into()),
                tool_calls: vec![],
            },
            ModelMessage::Tool {
                call_id: "c1".into(),
                output: serde_json::json!({
                    "service": "cmx-flow",
                    "count": 2,
                    "definitions": [{"name":"采购付款审批"},{"name":"报销审批"}]
                }),
            },
        ],
        tools: vec![],
    };
    let resp = DemoModel.complete(&ctx).await.unwrap();
    assert!(
        resp.tool_calls.is_empty(),
        "should finish, not call another tool"
    );
    let text = resp.text.unwrap();
    assert!(
        text.contains("采购付款审批"),
        "summary must mention real definition name: {text}"
    );
    assert!(text.contains('2'));
}
