//! Every background job registry, for the moment a document goes away.
//!
//! A job writes its result into the project that is open when it finishes,
//! so a job that outlives its project would write into the next one. Two
//! things stop that: each job captures the document's generation
//! (`AppState::generation`) and commits through `AppState::with_project_of`,
//! which drops a result for another generation; and closing a project
//! cancels the jobs that edit documents, so they also stop spending the
//! machine on a result nobody can use. Bakes (mattes, flow, enhance,
//! landmarks, isolated voices) are not cancelled: they fill caches keyed by
//! the file, never the document, and the next project that uses the file
//! finds them.

/// Cancel every job that would edit the document: tracking, the analyses
/// (scenes, stabilisation, beats, reframe, face and body landmarks) and the
/// project preparation.
pub fn cancel_all() {
    crate::modules::tracking::commands::cancel_all();
    crate::modules::analysis::jobs::cancel_all();
    crate::modules::prepare::commands::prepare_stop();
}
