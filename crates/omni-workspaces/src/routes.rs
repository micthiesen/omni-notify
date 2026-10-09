//! Workspace REST routes (`src/server.ts` lines 188-204 and 1669-1923).

use std::collections::HashMap;

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use omni_api::workspaces::{
    WorkspaceActionResponse, WorkspaceActionStatus, WorkspaceMessageAccepted, WorkspaceOverview,
    WorkspacePapercutResolution, WorkspacePapercutResponse, WorkspacePapercutStatus,
    WorkspacePapercutsResponse, WorkspaceResponse, WorkspaceSubjectResponse,
    WorkspaceSubjectStatus, WorkspaceSubjectUpdated, WorkspacesResponse,
};
use omni_server_kit::{ApiError, JsonBody, api_error};
use omni_tasks::RunNowError;
use serde_json::{Value, json};

use crate::entities::{ActionRow, PapercutRow, SubjectRow};
use crate::persistence::SubjectUpsert;
use crate::service::WorkspaceService;
use crate::text::{js_len, js_trim};

/// All workspace routes, state applied.
pub fn router(service: WorkspaceService) -> Router {
    Router::new()
        .route("/api/workspaces", get(list_workspaces))
        .route("/api/workspaces/{workspace_id}", get(get_workspace))
        .route(
            "/api/workspaces/{workspace_id}/subjects/{subject_id}",
            get(get_subject),
        )
        .route(
            "/api/workspaces/{workspace_id}/messages",
            post(post_message),
        )
        .route(
            "/api/workspaces/{workspace_id}/subjects/{subject_id}/status",
            post(post_subject_status),
        )
        .route("/api/workspace-actions/{action_id}/approve", post(approve))
        .route("/api/workspace-actions/{action_id}/reject", post(reject))
        .route("/api/workspace-papercuts", get(list_papercuts))
        .route(
            "/api/workspace-papercuts/{papercut_id}/resolve",
            post(resolve_papercut),
        )
        .with_state(service)
}

fn subjects(rows: &[SubjectRow]) -> Vec<omni_api::workspaces::WorkspaceSubject> {
    rows.iter().map(SubjectRow::view).collect()
}

fn actions(rows: &[ActionRow]) -> Vec<omni_api::workspaces::WorkspaceAction> {
    rows.iter().map(ActionRow::view).collect()
}

fn papercuts(rows: &[PapercutRow]) -> Vec<omni_api::workspaces::WorkspacePapercut> {
    rows.iter().map(PapercutRow::view).collect()
}

async fn list_workspaces(State(service): State<WorkspaceService>) -> Result<Response, ApiError> {
    let repo = service.repo();
    let mut workspaces = Vec::new();
    for definition in service.definitions() {
        let rows = repo
            .list_subjects(&definition.id)
            .await
            .map_err(ApiError::internal)?;
        let pending = repo
            .list_actions(&definition.id, None)
            .await
            .map_err(ApiError::internal)?
            .iter()
            .filter(|a| a.status == WorkspaceActionStatus::Pending)
            .count();
        let open = repo
            .list_papercuts(Some(&definition.id), Some(WorkspacePapercutStatus::Open))
            .await
            .map_err(ApiError::internal)?
            .len();
        workspaces.push(WorkspaceOverview {
            definition: definition.clone(),
            active_subject_count: rows
                .iter()
                .filter(|s| s.status == WorkspaceSubjectStatus::Active)
                .count() as u64,
            subjects: subjects(&rows),
            pending_action_count: pending as u64,
            open_papercut_count: open as u64,
        });
    }
    Ok(axum::Json(WorkspacesResponse { workspaces }).into_response())
}

async fn get_workspace(
    State(service): State<WorkspaceService>,
    Path(workspace_id): Path<String>,
) -> Result<Response, ApiError> {
    let Some(definition) = service.definition(&workspace_id) else {
        return Ok(api_error(StatusCode::NOT_FOUND, "Unknown workspace"));
    };
    let repo = service.repo();
    let rows = repo
        .list_subjects(&definition.id)
        .await
        .map_err(ApiError::internal)?;
    let action_rows = repo
        .list_actions(&definition.id, None)
        .await
        .map_err(ApiError::internal)?;
    let papercut_rows = repo
        .list_papercuts(Some(&definition.id), Some(WorkspacePapercutStatus::Open))
        .await
        .map_err(ApiError::internal)?;
    Ok(axum::Json(WorkspaceResponse {
        workspace: definition.clone(),
        subjects: subjects(&rows),
        actions: actions(&action_rows),
        papercuts: papercuts(&papercut_rows),
    })
    .into_response())
}

async fn get_subject(
    State(service): State<WorkspaceService>,
    Path((workspace_id, subject_id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let repo = service.repo();
    let subject = repo
        .get_subject(&workspace_id, &subject_id)
        .await
        .map_err(ApiError::internal)?;
    let (Some(definition), Some(subject)) = (service.definition(&workspace_id), subject) else {
        return Ok(api_error(
            StatusCode::NOT_FOUND,
            "Unknown workspace subject",
        ));
    };
    let artifacts = repo
        .latest_artifacts(&workspace_id, &subject_id)
        .await
        .map_err(ApiError::internal)?;
    let revisions = repo
        .list_artifact_revisions(&workspace_id, &subject_id, None)
        .await
        .map_err(ApiError::internal)?;
    let messages = repo
        .list_messages(&workspace_id, Some(&subject_id), 100)
        .await
        .map_err(ApiError::internal)?;
    let sources = repo
        .list_sources(&workspace_id, &subject_id, 100)
        .await
        .map_err(ApiError::internal)?;
    let action_rows = repo
        .list_actions(&workspace_id, Some(&subject_id))
        .await
        .map_err(ApiError::internal)?;
    let scope = repo
        .get_email_scope(&workspace_id, &subject_id)
        .await
        .map_err(ApiError::internal)?;
    let papercut_rows: Vec<PapercutRow> = repo
        .list_papercuts(Some(&workspace_id), Some(WorkspacePapercutStatus::Open))
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .filter(|p| {
            p.subject_id
                .as_deref()
                .is_none_or(|s| s.is_empty() || s == subject_id)
        })
        .collect();
    Ok(axum::Json(WorkspaceSubjectResponse {
        workspace: definition.clone(),
        subject: subject.view(),
        artifacts: artifacts.iter().map(|a| a.view()).collect(),
        artifact_revisions: revisions.iter().map(|a| a.view()).collect(),
        messages: messages.iter().map(|m| m.view()).collect(),
        sources: sources.iter().map(|s| s.view()).collect(),
        actions: actions(&action_rows),
        email_scope: scope.map(|s| s.scope()),
        papercuts: papercuts(&papercut_rows),
    })
    .into_response())
}

/// `{message: trimmed, 1..=20000 chars, subjectId?: non-empty}`.
fn parse_message_body(body: &Value) -> Option<(String, Option<String>)> {
    let object = body.as_object()?;
    let message = object.get("message")?.as_str()?;
    if js_trim(message) != message || message.is_empty() || js_len(message) > 20_000 {
        return None;
    }
    let subject_id = match object.get("subjectId") {
        None => None,
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(_) => return None,
    };
    Some((message.to_owned(), subject_id))
}

async fn post_message(
    State(service): State<WorkspaceService>,
    Path(workspace_id): Path<String>,
    JsonBody(body): JsonBody<Value>,
) -> Result<Response, ApiError> {
    let Some(definition) = service.definition(&workspace_id) else {
        return Ok(api_error(StatusCode::NOT_FOUND, "Unknown workspace"));
    };
    let Some((message, subject_id)) = parse_message_body(&body) else {
        return Ok(api_error(StatusCode::BAD_REQUEST, "A message is required"));
    };
    if let Some(subject_id) = &subject_id
        && service
            .repo()
            .get_subject(&definition.id, subject_id)
            .await
            .map_err(ApiError::internal)?
            .is_none()
    {
        return Ok(api_error(
            StatusCode::NOT_FOUND,
            "Unknown workspace subject",
        ));
    }
    let mut input = json!({ "message": message });
    if let Some(subject_id) = subject_id {
        input["subjectId"] = Value::String(subject_id);
    }
    match service.tasks().run_now(&definition.task_name, Some(input)) {
        Ok(run_id) => Ok((
            StatusCode::ACCEPTED,
            axum::Json(WorkspaceMessageAccepted { run_id }),
        )
            .into_response()),
        Err(RunNowError::AlreadyRunning { .. }) => Ok(api_error(
            StatusCode::CONFLICT,
            "Workspace agent is already running",
        )),
        Err(RunNowError::NotFound { .. }) => Ok(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Workspace task is unavailable",
        )),
        Err(error) => Err(ApiError::internal(error)),
    }
}

fn parse_status(body: &Value) -> Option<WorkspaceSubjectStatus> {
    serde_json::from_value(body.get("status")?.clone()).ok()
}

async fn post_subject_status(
    State(service): State<WorkspaceService>,
    Path((workspace_id, subject_id)): Path<(String, String)>,
    JsonBody(body): JsonBody<Value>,
) -> Result<Response, ApiError> {
    let repo = service.repo();
    let Some(subject) = repo
        .get_subject(&workspace_id, &subject_id)
        .await
        .map_err(ApiError::internal)?
    else {
        return Ok(api_error(
            StatusCode::NOT_FOUND,
            "Unknown workspace subject",
        ));
    };
    let Some(status) = parse_status(&body) else {
        return Ok(api_error(
            StatusCode::BAD_REQUEST,
            "A valid status is required",
        ));
    };
    let updated = repo
        .upsert_subject(SubjectUpsert {
            workspace_id: subject.workspace_id,
            subject_id: subject.subject_id,
            title: subject.title,
            status,
            summary: subject.summary,
            created_at: Some(subject.created_at),
            updated_at: None,
            last_researched_at: subject.last_researched_at,
        })
        .await
        .map_err(ApiError::internal)?;
    Ok(axum::Json(WorkspaceSubjectUpdated {
        subject: updated.view(),
    })
    .into_response())
}

async fn approve(
    State(service): State<WorkspaceService>,
    Path(action_id): Path<String>,
) -> Response {
    match service.approve_action(&action_id).await {
        Ok(action) => axum::Json(WorkspaceActionResponse {
            action: action.view(),
        })
        .into_response(),
        Err(error) => api_error(StatusCode::CONFLICT, error.to_string()),
    }
}

async fn reject(
    State(service): State<WorkspaceService>,
    Path(action_id): Path<String>,
) -> Response {
    match service.reject_action(&action_id).await {
        Ok(action) => axum::Json(WorkspaceActionResponse {
            action: action.view(),
        })
        .into_response(),
        Err(error) => api_error(StatusCode::CONFLICT, error.to_string()),
    }
}

async fn list_papercuts(
    State(service): State<WorkspaceService>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let status = query
        .get("status")
        .and_then(|s| serde_json::from_value::<WorkspacePapercutStatus>(json!(s)).ok());
    let rows = service
        .repo()
        .list_papercuts(query.get("workspaceId").map(String::as_str), status)
        .await
        .map_err(ApiError::internal)?;
    Ok(axum::Json(WorkspacePapercutsResponse {
        papercuts: papercuts(&rows),
    })
    .into_response())
}

fn parse_resolution(body: &Value) -> Option<(WorkspacePapercutResolution, String)> {
    let status = serde_json::from_value(body.get("status")?.clone()).ok()?;
    let resolution = body.get("resolution")?.as_str()?;
    (js_trim(resolution) == resolution && !resolution.is_empty() && js_len(resolution) <= 2_000)
        .then(|| (status, resolution.to_owned()))
}

async fn resolve_papercut(
    State(service): State<WorkspaceService>,
    Path(papercut_id): Path<String>,
    JsonBody(body): JsonBody<Value>,
) -> Result<Response, ApiError> {
    let Some((status, resolution)) = parse_resolution(&body) else {
        return Ok(api_error(
            StatusCode::BAD_REQUEST,
            "Status and resolution are required",
        ));
    };
    let Some(papercut) = service
        .repo()
        .resolve_papercut(&papercut_id, status.into(), &resolution)
        .await
        .map_err(ApiError::internal)?
    else {
        return Ok(api_error(StatusCode::NOT_FOUND, "Unknown papercut"));
    };
    Ok(axum::Json(WorkspacePapercutResponse {
        papercut: papercut.view(),
    })
    .into_response())
}
