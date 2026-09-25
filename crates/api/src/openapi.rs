use rig_domain::*;
use schemars::{JsonSchema, schema_for};
use serde_json::{Map, Value, json};

fn schema<T: JsonSchema>(schemas: &mut Map<String, Value>, name: &str) {
    let mut root = serde_json::to_value(schema_for!(T)).unwrap();
    if let Some(defs) = root.as_object_mut().unwrap().remove("$defs") {
        for (key, value) in defs.as_object().unwrap() {
            schemas.insert(key.clone(), value.clone());
        }
    }
    root.as_object_mut().unwrap().remove("$schema");
    schemas.insert(name.into(), root);
}
fn reference(name: &str) -> Value {
    json!({"$ref":format!("#/components/schemas/{name}")})
}
fn operation(input: Option<&str>, output: &str, array: bool) -> Value {
    let output = if array {
        json!({"type":"array","items":reference(output)})
    } else {
        reference(output)
    };
    let mut result = json!({"responses":{"200":{"description":"Success","content":{"application/json":{"schema":output}}},"400":{"description":"Validation error"},"401":{"description":"Authentication required"},"403":{"description":"Forbidden or invalid CSRF"},"404":{"description":"Not found"},"409":{"description":"Conflict"},"503":{"description":"Dependency unavailable"}},"security":[{"session":[]},{"bearer":[]}]});
    if let Some(input) = input {
        result["requestBody"] =
            json!({"required":true,"content":{"application/json":{"schema":reference(input)}}});
    }
    result
}
pub fn document() -> Value {
    let mut schemas = Map::new();
    schema::<Login>(&mut schemas, "Login");
    schema::<BiosSnapshot>(&mut schemas, "BiosSnapshot");
    schema::<Session>(&mut schemas, "Session");
    schema::<MessageDismiss>(&mut schemas, "MessageDismiss");
    schema::<Machine>(&mut schemas, "Machine");
    schema::<MachineInput>(&mut schemas, "MachineInput");
    schema::<SshProbeInput>(&mut schemas, "SshProbeInput");
    schema::<SshHostKey>(&mut schemas, "SshHostKey");
    schema::<CatalogItem>(&mut schemas, "CatalogItem");
    schema::<CatalogInput>(&mut schemas, "CatalogInput");
    schema::<FlightSheet>(&mut schemas, "FlightSheet");
    schema::<FlightInput>(&mut schemas, "FlightInput");
    schema::<Job>(&mut schemas, "Job");
    schema::<JobInput>(&mut schemas, "JobInput");
    schema::<Observation>(&mut schemas, "Observation");
    schema::<AuditEvent>(&mut schemas, "AuditEvent");
    schema::<AdapterDescription>(&mut schemas, "AdapterDescription");
    schema::<TokenInfo>(&mut schemas, "TokenInfo");
    schema::<ResolveTarget>(&mut schemas, "ResolveTarget");
    schema::<MachineMessage>(&mut schemas, "MachineMessage");
    schema::<MinerLog>(&mut schemas, "MinerLog");
    schema::<SshProbeBatch>(&mut schemas, "SshProbeBatch");
    schema::<SshProbeResult>(&mut schemas, "SshProbeResult");
    schema::<MachineBatchInput>(&mut schemas, "MachineBatchInput");
    schema::<MachineBatchResult>(&mut schemas, "MachineBatchResult");
    schemas.insert("JobAccepted".into(),json!({"type":"object","required":["job_id"],"properties":{"job_id":{"type":"string","format":"uuid"}}}));
    schemas.insert(
        "Object".into(),
        json!({"type":"object","additionalProperties":true}),
    );
    let mut paths = Map::new();
    paths.insert(
        "/api/v1/ssh/host-key".into(),
        json!({"post":operation(Some("SshProbeInput"),"SshHostKey",false)}),
    );
    paths.insert(
        "/api/v1/ssh/host-keys".into(),
        json!({"post":{"requestBody":{"required":true,"content":{"application/json":{"schema":reference("SshProbeBatch")}}},"responses":{"200":{"description":"Success","content":{"application/json":{"schema":{"type":"array","items":reference("SshProbeResult")}}}}}}}),
    );
    paths.insert(
        "/api/v1/machines/batch".into(),
        json!({"post":operation(Some("MachineBatchInput"),"MachineBatchResult",false)}),
    );
    for (path, input, output) in [
        ("/machines", "MachineInput", "Machine"),
        ("/catalog/{kind}", "CatalogInput", "CatalogItem"),
        ("/flight-sheets", "FlightInput", "FlightSheet"),
    ] {
        paths.insert(
            format!("/api/v1{path}"),
            json!({"get":operation(None,output,true),"post":operation(Some(input),output,false)}),
        );
        paths.insert(format!("/api/v1{path}/{{id}}"),json!({"put":operation(Some(input),output,false),"delete":{"responses":{"204":{"description":"Deleted"}}}}));
    }
    for (path, output, array) in [
        ("/me", "Session", false),
        ("/jobs", "Job", true),
        ("/jobs/{id}", "Job", false),
        ("/telemetry", "Observation", true),
        ("/machines/{id}/history", "Observation", true),
        ("/machines/{id}/bios", "BiosSnapshot", false),
        ("/machines/{id}/bmc/{kind}", "Object", false),
        ("/machines/{id}/messages", "MachineMessage", true),
        ("/machines/{id}/instances/{instance}/log", "MinerLog", false),
        ("/audit", "AuditEvent", true),
        ("/adapters", "AdapterDescription", true),
        ("/tokens", "TokenInfo", true),
    ] {
        paths.entry(format!("/api/v1{path}")).or_insert(json!({}))["get"] =
            operation(None, output, array);
    }
    paths.get_mut("/api/v1/jobs").unwrap()["post"] =
        operation(Some("JobInput"), "JobAccepted", false);
    paths.insert(
        "/api/v1/login".into(),
        json!({"post":operation(Some("Login"),"Session",false)}),
    );
    for (path, method) in [
        ("/logout", "post"),
        ("/jobs/{id}/cancel", "post"),
        ("/machines/{id}/test", "post"),
        ("/tokens/{id}", "delete"),
    ] {
        paths.insert(
            format!("/api/v1{path}"),
            json!({method:operation(None,"Object",false)}),
        );
    }
    paths.get_mut("/api/v1/tokens").unwrap()["post"] = operation(Some("Object"), "Object", false);
    paths.insert(
        "/api/v1/machines/{id}/messages/dismiss".into(),
        json!({"post":operation(Some("MessageDismiss"),"Object",false)}),
    );
    paths.insert(
        "/api/v1/job-targets/{id}/resolve".into(),
        json!({"post":operation(Some("ResolveTarget"),"Object",false)}),
    );
    paths.insert("/api/v1/packages".into(),json!({"post":{"requestBody":{"required":true,"content":{"application/octet-stream":{"schema":{"type":"string","format":"binary"}}}},"responses":{"200":{"description":"Content addressed artifact","content":{"application/json":{"schema":reference("Object")}}}}}}));
    paths["/api/v1/login"]["post"]["security"] = json!([]);
    for (path, method, status) in [
        ("/api/v1/logout", "post", "204"),
        ("/api/v1/tokens/{id}", "delete", "204"),
        ("/api/v1/jobs/{id}/cancel", "post", "204"),
        ("/api/v1/job-targets/{id}/resolve", "post", "202"),
        ("/api/v1/machines/{id}/messages/dismiss", "post", "204"),
    ] {
        let responses = paths.get_mut(path).unwrap()[method]["responses"]
            .as_object_mut()
            .unwrap();
        responses.remove("200");
        responses.insert(
            status.into(),
            json!({"description":"Accepted without response body"}),
        );
    }
    for (path, item) in &mut paths {
        for (method, operation) in item.as_object_mut().unwrap() {
            if method != "parameters" && path != "/api/v1/login" {
                operation["security"] = json!([{"session":[]},{"bearer":[]}]);
            }
        }
    }
    for (path, item) in &mut paths {
        let mut params = Vec::new();
        for name in ["kind", "id", "instance"] {
            if path.contains(&format!("{{{name}}}")) {
                params.push(
                    json!({"name":name,"in":"path","required":true,"schema":{"type":"string"}}),
                );
            }
        }
        if path.ends_with("/telemetry") {
            params.push(
                json!({"name":"summary","in":"query","schema":{"type":"boolean","default":false}}),
            );
            params.push(json!({"name":"machine_id","in":"query","schema":{"type":"string","format":"uuid"}}));
        }
        if path.ends_with("/messages") {
            params.push(json!({"name":"limit","in":"query","schema":{"type":"integer"}}));
            params.push(
                json!({"name":"unread","in":"query","schema":{"type":"boolean","default":false}}),
            );
        }
        if path.ends_with("/log") {
            params.push(json!({"name":"lines","in":"query","schema":{"type":"integer"}}));
        }
        if path.ends_with("/history") {
            params.push(
                json!({"name":"summary","in":"query","schema":{"type":"boolean","default":false}}),
            );
            params.push(
                json!({"name":"kind","in":"query","required":true,"schema":{"type":"string"}}),
            );
            params.push(json!({"name":"hours","in":"query","schema":{"type":"integer"}}));
        }
        if !params.is_empty() {
            item["parameters"] = json!(params);
        }
    }
    let doc = json!({"openapi":"3.1.0","info":{"title":"RigDeck API","version":"0.1.0"},"paths":paths,"components":{"schemas":schemas,"securitySchemes":{"session":{"type":"apiKey","in":"cookie","name":"rigdeck_session"},"bearer":{"type":"http","scheme":"bearer"}}}});
    serde_json::from_str(
        &serde_json::to_string(&doc)
            .unwrap()
            .replace("#/$defs/", "#/components/schemas/"),
    )
    .unwrap()
}
