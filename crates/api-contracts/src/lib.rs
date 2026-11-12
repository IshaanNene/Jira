//! Generated gRPC contracts for internal service-to-service calls.
//! Source of truth: `proto/`. Fields are only ever added, never renumbered or reused.

#![allow(clippy::all, clippy::pedantic)]

pub mod tenant {
    pub mod v1 {
        tonic::include_proto!("platform.tenant.v1");
    }
}

pub mod user {
    pub mod v1 {
        tonic::include_proto!("platform.user.v1");
    }
}

pub mod permission {
    pub mod v1 {
        tonic::include_proto!("platform.permission.v1");
    }
}

pub mod project {
    pub mod v1 {
        tonic::include_proto!("platform.project.v1");
    }
}

pub mod workflow {
    pub mod v1 {
        tonic::include_proto!("platform.workflow.v1");
    }
}

pub mod issue {
    pub mod v1 {
        tonic::include_proto!("platform.issue.v1");
    }
}
