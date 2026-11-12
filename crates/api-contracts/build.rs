fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = "../../proto";
    let protos = [
        "platform/tenant/v1/tenant.proto",
        "platform/user/v1/membership.proto",
        "platform/permission/v1/permission.proto",
        "platform/project/v1/project.proto",
        "platform/workflow/v1/workflow.proto",
        "platform/issue/v1/issue.proto",
    ];
    for proto in protos {
        println!("cargo:rerun-if-changed={root}/{proto}");
    }
    let paths: Vec<String> = protos.iter().map(|p| format!("{root}/{p}")).collect();
    tonic_build::configure().build_client(true).build_server(true).compile_protos(&paths, &[root])?;
    Ok(())
}
