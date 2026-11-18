use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext}, storage::Store,
    tool_dispatch::{AuthenticatedEditorScope, dispatch_editor_native_tool},
    transparent::TextEdit,
};

fn context(actor: &str) -> OperationContext {
    OperationContext { actor:actor.into(), allow_write:true, ..Default::default() }
}

#[tokio::test]
async fn native_editor_invocation_commits_original_parameters_with_scope_and_version() {
    let core = Core::new(Store::memory().await.unwrap());
    let auth = context("editor-owner");
    let request = json!({"model":"host-model","stream":true,"n":2,"temperature":0.3,
        "messages":[{"role":"user","content":"first candidate"},{"role":"assistant","content":"other unit"}],
        "x-harness":{"route":"original"}});
    let begun = core.operate("native.begin", &auth, &json!({"format":"openai-chat","request":request})).await.unwrap();
    let source = &begun["result"];
    let unit = source["units"][0]["unit_id"].as_str().unwrap();
    let scope = AuthenticatedEditorScope::from_authenticated_host(&auth,
        source["source_id"].as_str().unwrap(),source["source_digest"].as_str().unwrap(),0,
        vec![unit.into()],vec!["apply_patch".into()]).unwrap();
    let args = json!({"patch":"replace first candidate with edited candidate","path":"candidate://current"});
    let committed = dispatch_editor_native_tool(&core,&auth,&scope,"apply_patch",&args,|name, original| {
        assert_eq!(name,"apply_patch");
        assert_eq!(original,&args);
        Ok(vec![TextEdit{unit_id:unit.into(),expected_text:"first candidate".into(),replacement:"edited candidate".into()}])
    }).await.unwrap();
    assert_eq!(committed["result"]["version"],1);
    let mut expected = request.clone();
    expected["messages"][0]["content"] = json!("edited candidate");
    assert_eq!(committed["result"]["request"],expected);
    assert_eq!(committed["result"]["source_request"],request);
    assert_eq!(committed["result"]["receipt"]["native_tool"]["name"],"apply_patch");
    assert_eq!(committed["result"]["receipt"]["native_tool"]["arguments_sha256"],sic_core_rust::storage::digest(&args).unwrap());
    let retry = dispatch_editor_native_tool(&core,&auth,&scope,"apply_patch",&args,|_,_|Ok(vec![])).await.unwrap_err();
    assert_eq!(retry.status,409);
    let recovered = core.operate("native.get",&auth,&json!({"source_id":source["source_id"]})).await.unwrap();
    assert_eq!(recovered["result"]["version"],1);
    assert_eq!(recovered["result"]["request"],expected);
}

#[tokio::test]
async fn editor_cannot_dispatch_task_tools_cross_actor_or_unassigned_candidate_units() {
    let core = Core::new(Store::memory().await.unwrap());
    let auth = context("editor-owner");
    let scope = AuthenticatedEditorScope::from_authenticated_host(&auth,"candidate","digest",0,
        vec!["assigned".into()],vec!["apply_patch".into()]).unwrap();
    for (caller,name) in [(auth.clone(),"exec_command"),(context("other-actor"),"apply_patch")] {
        let error = dispatch_editor_native_tool(&core,&caller,&scope,name,&Value::Null,
            |_,_|panic!("unauthorized native invocation reached the Harness mapper")).await.unwrap_err();
        assert_eq!(error.status,403);
    }
    let error = dispatch_editor_native_tool(&core,&auth,&scope,"apply_patch",&json!({}),|_,_| {
        Ok(vec![TextEdit{unit_id:"prior-cb-unit".into(),expected_text:"x".into(),replacement:"y".into()}])
    }).await.unwrap_err();
    assert_eq!(error.status,403);
    let mut denied = auth;
    denied.allow_write = false;
    assert!(AuthenticatedEditorScope::from_authenticated_host(&denied,"candidate","digest",0,
        vec!["assigned".into()],vec!["apply_patch".into()]).is_err());
}
