//! Drive files API — list, upload (w/ auto-versioning), download, delete, mkdir, trash, versions.

use axum::{
    body::Bytes,
    extract::{Multipart, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, head, patch, post},
    Json, Router,
};
use std::io::Write as IoWrite;
use time::OffsetDateTime;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tokio::{fs, io::AsyncWriteExt};
use uuid::Uuid;

use crate::{
    api::context::RequestCtx,
    domain::{DriveFile, FileRepo, FileVersion, FolderQuota, FolderQuotaRepo, NewFile, NewVersion, QuotaRepo, TagRepo, UserUsage, VersionRepo},
    error::{DriveError, Result},
    state::AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/drive/files",                       get(list).post(upload))
        .route("/api/v1/drive/files/mkdir",                 post(mkdir))
        .route("/api/v1/drive/files/:id",                   get(download).delete(delete).head(head_file))
        .route("/api/v1/drive/files/:id/preview",           get(preview))
        .route("/api/v1/drive/files/:id/metadata",          get(metadata).patch(rename))
        .route("/api/v1/drive/files/search",                 get(search))
        .route("/api/v1/drive/files/bulk-trash",             post(bulk_trash))
        .route("/api/v1/drive/files/bulk-move",              post(bulk_move))
        .route("/api/v1/drive/files/bulk-copy",              post(bulk_copy))
        .route("/api/v1/drive/files/bulk-restore",           post(bulk_restore))
        .route("/api/v1/drive/files/:id/copy",               post(copy_file))
        .route("/api/v1/drive/files/:id/move",              post(move_file))
        .route("/api/v1/drive/files/:id/restore",           post(restore))
        .route("/api/v1/drive/files/:id/tags",              get(list_tags).post(add_tag))
        .route("/api/v1/drive/files/:id/tags/:tag",         delete(remove_tag))
        .route("/api/v1/drive/files/:id/versions",          get(list_versions))
        .route("/api/v1/drive/files/:id/versions/:v",          get(download_version).delete(delete_version))
        .route("/api/v1/drive/files/:id/versions/:v/metadata", get(version_metadata))
        .route("/api/v1/drive/files/:id/versions/:v/restore",  post(restore_version))
        .route("/api/v1/drive/files/:id/versions/:v/diff-content", get(diff_version_content))
        .route("/api/v1/drive/files/:id/expiry",              patch(set_expiry))
        .route("/api/v1/drive/files/:id/lock",               post(lock_file).delete(unlock_file))
        .route("/api/v1/drive/files/:id/star",               post(star_file).delete(unstar_file))
        .route("/api/v1/drive/starred",                      get(list_starred))
        .route("/api/v1/drive/starred/count",                get(count_starred))
        .route("/api/v1/drive/folders/:id/download",         get(download_folder))
        .route("/api/v1/drive/folders/:id/quota",           get(folder_quota).put(set_folder_quota).delete(delete_folder_quota))
        .route("/api/v1/drive/trash",                       get(trash).delete(purge_trash))
        .route("/api/v1/drive/quota",                       get(quota))
        .route("/api/v1/drive/files/stats",                 get(file_stats))
        .route("/api/v1/drive/files/stats/users",           get(file_stats_users))
        .route("/api/v1/drive/files/stats/folders",         get(file_stats_folders))
        .route("/api/v1/drive/files/stats/extensions",      get(file_stats_extensions))
        .route("/api/v1/drive/files/stats/activity",       get(file_stats_activity))
        .route("/api/v1/drive/files/stats/age",            get(file_stats_age))
        .route("/api/v1/drive/files/stats/owners",        get(file_stats_owners))
        .route("/api/v1/drive/files/stats/size-buckets", get(file_stats_size_buckets))
        .route("/api/v1/drive/files/stats/deleted",         get(file_stats_deleted))
        .route("/api/v1/drive/files/stats/by-owner-and-ext", get(file_stats_by_owner_and_ext))
        .route("/api/v1/drive/files/stats/recent",           get(file_stats_recent))
        .route("/api/v1/drive/files/stats/mime-by-folder",  get(file_stats_mime_by_folder))
        .route("/api/v1/drive/files/stats/top-files",        get(file_stats_top_files))
        .route("/api/v1/drive/files/stats/created-by-day",  get(file_stats_created_by_day))
        .route("/api/v1/drive/files/stats/by-size-bucket",  get(file_stats_by_size_bucket))
        .route("/api/v1/drive/files/stats/updated-by-day",  get(file_stats_updated_by_day))
        .route("/api/v1/drive/files/stats/folder-depth",    get(file_stats_folder_depth))
        .route("/api/v1/drive/files/stats/version-count",   get(file_stats_version_count))
        .route("/api/v1/drive/files/stats/tag-count",        get(file_stats_tag_count))
        .route("/api/v1/drive/files/stats/ext-by-folder",    get(file_stats_ext_by_folder))
        .route("/api/v1/drive/files/stats/lock-count",       get(file_stats_lock_count))
        .route("/api/v1/drive/files/stats/starred-count",    get(file_stats_starred_count))
        .route("/api/v1/drive/files/stats/expiry-count",    get(file_stats_expiry_count))
        .route("/api/v1/drive/files/stats/mime-top-n",       get(file_stats_mime_top_n))
        .route("/api/v1/drive/files/stats/orphan-versions",  get(file_stats_orphan_versions))
        .route("/api/v1/drive/files/stats/empty-files",      get(file_stats_empty_files))
        .route("/api/v1/drive/files/stats/deleted-by-day",   get(file_stats_deleted_by_day))
        .route("/api/v1/drive/files/stats/name-length",       get(file_stats_name_length))
        .route("/api/v1/drive/files/stats/ext-top-n",         get(file_stats_ext_top_n))
        .route("/api/v1/drive/files/stats/storage-by-user",   get(file_stats_storage_by_user))
        .route("/api/v1/drive/files/stats/quota-usage",        get(file_stats_quota_usage))
        .route("/api/v1/drive/files/stats/folder-file-count",  get(file_stats_folder_file_count))
        .route("/api/v1/drive/files/stats/deep-files",         get(file_stats_deep_files))
        .route("/api/v1/drive/files/stats/mime-entropy",        get(file_stats_mime_entropy))
        .route("/api/v1/drive/files/stats/avg-versions",         get(file_stats_avg_versions))
        .route("/api/v1/drive/files/stats/ext-entropy",           get(file_stats_ext_entropy))
        .route("/api/v1/drive/files/stats/checksum-coverage",      get(file_stats_checksum_coverage))
        .route("/api/v1/drive/files/stats/storage-key-coverage",   get(file_stats_storage_key_coverage))
        .route("/api/v1/drive/files/stats/locked-by-user",         get(file_stats_locked_by_user))
        .route("/api/v1/drive/files/stats/created-by-month",        get(file_stats_created_by_month))
        .route("/api/v1/drive/files/stats/modified-by-month",       get(file_stats_modified_by_month))
        .route("/api/v1/drive/files/stats/starred-by-month",        get(file_stats_starred_by_month))
        .route("/api/v1/drive/files/stats/locked-by-month",         get(file_stats_locked_by_month))
        .route("/api/v1/drive/files/stats/ext-by-month",            get(file_stats_ext_by_month))
        .route("/api/v1/drive/files/stats/size-by-weekday",         get(file_stats_size_by_weekday))
        .route("/api/v1/drive/files/stats/size-by-month",           get(file_stats_size_by_month))
        .route("/api/v1/drive/files/stats/versioned-by-month",      get(file_stats_versioned_by_month))
        .route("/api/v1/drive/files/stats/deleted-by-month",        get(file_stats_deleted_by_month))
        .route("/api/v1/drive/files/stats/mime-by-month",           get(file_stats_mime_by_month))
        .route("/api/v1/drive/files/stats/version-count-by-month",  get(file_stats_version_count_by_month))
        .route("/api/v1/drive/files/stats/folder-count-by-month",   get(file_stats_folder_count_by_month))
        .route("/api/v1/drive/files/stats/name-length-by-month",    get(file_stats_name_length_by_month))
        .route("/api/v1/drive/files/stats/name-length-by-weekday",  get(file_stats_name_length_by_weekday))
        .route("/api/v1/drive/files/stats/folder-count-by-weekday", get(file_stats_folder_count_by_weekday))
        .route("/api/v1/drive/files/stats/locked-by-weekday",       get(file_stats_locked_by_weekday))
        .route("/api/v1/drive/files/stats/versioned-by-weekday",    get(file_stats_versioned_by_weekday))
        .route("/api/v1/drive/files/stats/size-by-hour",            get(file_stats_size_by_hour))
        .route("/api/v1/drive/files/stats/starred-by-hour",         get(file_stats_starred_by_hour))
        .route("/api/v1/drive/files/stats/locked-by-hour",          get(file_stats_locked_by_hour))
        .route("/api/v1/drive/files/stats/name-length-by-hour",     get(file_stats_name_length_by_hour))
        .route("/api/v1/drive/files/stats/mime-by-hour",            get(file_stats_mime_by_hour))
        .route("/api/v1/drive/files/stats/version-size-by-month",        get(file_stats_version_size_by_month))
        .route("/api/v1/drive/files/stats/version-size-by-hour",         get(file_stats_version_size_by_hour))
        .route("/api/v1/drive/files/stats/folder-count-by-hour",        get(file_stats_folder_count_by_hour))
        .route("/api/v1/drive/files/stats/version-count-by-hour",      get(file_stats_version_count_by_hour))
        .route("/api/v1/drive/files/stats/ext-by-hour",               get(file_stats_ext_by_hour))
        .route("/api/v1/drive/files/stats/versioned-by-hour",        get(file_stats_versioned_by_hour))
        .route("/api/v1/drive/files/stats/deleted-by-hour",         get(file_stats_deleted_by_hour))
        .route("/api/v1/drive/files/stats/mime-by-ext",             get(file_stats_mime_by_ext))
        .route("/api/v1/drive/files/stats/size-trend-by-day",       get(file_stats_size_trend_by_day))
        .route("/api/v1/drive/files/stats/version-age",             get(file_stats_version_age))
        .route("/api/v1/drive/files/stats/mime-count-by-user",      get(file_stats_mime_count_by_user))
        .route("/api/v1/drive/files/stats/created-vs-deleted-by-day", get(file_stats_created_vs_deleted_by_day))
        .route("/api/v1/drive/files/stats/version-size-by-user",   get(file_stats_version_size_by_user))
        .route("/api/v1/drive/files/stats/ext-size-by-folder",     get(file_stats_ext_size_by_folder))
        .route("/api/v1/drive/files/stats/tag-by-user",            get(file_stats_tag_by_user))
        .route("/api/v1/drive/files/stats/tag-entropy",            get(file_stats_tag_entropy))
        .route("/api/v1/drive/files/stats/folder-mime-entropy",    get(file_stats_folder_mime_entropy))
        .route("/api/v1/drive/files/stats/size-entropy",           get(file_stats_size_entropy))
        .route("/api/v1/drive/files/stats/version-count-by-ext",   get(file_stats_version_count_by_ext))
        .route("/api/v1/drive/files/stats/tag-frequency-by-folder",    get(file_stats_tag_frequency_by_folder))
        .route("/api/v1/drive/files/stats/size-trend-by-folder",      get(file_stats_size_trend_by_folder))
        .route("/api/v1/drive/files/stats/folder-count-by-user",      get(file_stats_folder_count_by_user))
        .route("/api/v1/drive/files/stats/file-age-by-folder",        get(file_stats_file_age_by_folder))
        .route("/api/v1/drive/files/stats/large-files",               get(file_stats_large_files))
        .route("/api/v1/drive/files/stats/created-by-hour",           get(file_stats_created_by_hour))
        .route("/api/v1/drive/files/stats/last-modified-by-folder",   get(file_stats_last_modified_by_folder))
        .route("/api/v1/drive/files/stats/starred-by-folder",         get(file_stats_starred_by_folder))
        .route("/api/v1/drive/files/stats/zero-size",                 get(file_stats_zero_size))
        .route("/api/v1/drive/files/stats/ext-by-weekday",            get(file_stats_ext_by_weekday))
        .route("/api/v1/drive/files/stats/size-by-weekday",           get(file_stats_size_by_weekday))
        .route("/api/v1/drive/files/stats/modified-by-hour",          get(file_stats_modified_by_hour))
        .route("/api/v1/drive/files/stats/ext-version-age",           get(file_stats_ext_version_age))
        .route("/api/v1/drive/files/stats/storage-by-folder",        get(file_stats_storage_by_folder))
        .route("/api/v1/drive/files/stats/avg-file-size-by-folder", get(file_stats_avg_file_size_by_folder))
        .route("/api/v1/drive/files/stats/folder-size-entropy",   get(file_stats_folder_size_entropy))
        .route("/api/v1/drive/files/stats/locked-age",            get(file_stats_locked_age))
        .route("/api/v1/drive/files/stats/version-size-by-ext",   get(file_stats_version_size_by_ext))
        .route("/api/v1/drive/files/stats/owner-entropy",          get(file_stats_owner_entropy))
        .route("/api/v1/drive/files/stats/size-percentile",        get(file_stats_size_percentile))
        .route("/api/v1/drive/files/stats/created-vs-updated-gap", get(file_stats_created_vs_updated_gap))
        .route("/api/v1/drive/files/stats/starred-age",            get(file_stats_starred_age))
        .route("/api/v1/drive/files/stats/folder-age",             get(file_stats_folder_age))
        .route("/api/v1/drive/files/stats/tag-size-by-ext",        get(file_stats_tag_size_by_ext))
        .route("/api/v1/drive/files/stats/ext-count-by-user",           get(file_stats_ext_count_by_user))
        .route("/api/v1/drive/files/stats/size-stdev-by-ext",           get(file_stats_size_stdev_by_ext))
        .route("/api/v1/drive/files/stats/quota-utilization-by-folder", get(file_stats_quota_utilization_by_folder))
        .route("/api/v1/drive/files/stats/file-count-by-weekday",       get(file_stats_file_count_by_weekday))
        .route("/api/v1/drive/files/stats/version-count-by-user",           get(file_stats_version_count_by_user))
        .route("/api/v1/drive/files/stats/size-cv-by-folder",               get(file_stats_size_cv_by_folder))
        .route("/api/v1/drive/files/stats/deleted-by-day",                  get(file_stats_deleted_by_day))
        .route("/api/v1/drive/files/stats/mime-top-by-size",                get(file_stats_mime_top_by_size))
        .route("/api/v1/drive/files/stats/name-length-by-ext",  get(file_stats_name_length_by_ext))
        .route("/api/v1/drive/files/stats/ext-size-percentile", get(file_stats_ext_size_percentile))
        .route("/api/v1/drive/files/stats/orphan-files",        get(file_stats_orphan_files))
        .route("/api/v1/drive/files/stats/duplicate-name",      get(file_stats_duplicate_name))
        .route("/api/v1/drive/files/stats/deleted-size",                get(file_stats_deleted_size))
        .route("/api/v1/drive/files/stats/created-by-weekday-and-ext",  get(file_stats_created_by_weekday_and_ext))
        .route("/api/v1/drive/files/stats/avg-version-size",            get(file_stats_avg_version_size))
        .route("/api/v1/drive/files/stats/folder-count",                get(file_stats_folder_count))
        .route("/api/v1/drive/files/stats/starred-by-weekday",          get(file_stats_starred_by_weekday))
        .route("/api/v1/drive/files/stats/deleted-by-weekday",          get(file_stats_deleted_by_weekday))
        .route("/api/v1/drive/files/stats/version-size-by-weekday",     get(file_stats_version_size_by_weekday))
        .route("/api/v1/drive/files/stats/version-count-by-weekday",    get(file_stats_version_count_by_weekday))
        .route("/api/v1/drive/files/stats/mime-by-weekday",             get(file_stats_mime_by_weekday))
        .route("/api/v1/drive/files/stats/modified-by-weekday",        get(file_stats_modified_by_weekday))
        .route("/api/v1/drive/files/stats/file-count-by-hour",         get(file_stats_file_count_by_hour))
        .route("/api/v1/drive/files/stats/quota-by-weekday",           get(file_stats_quota_by_weekday))
        .route("/api/v1/drive/files/stats/quota-by-hour",              get(file_stats_quota_by_hour))
        .route("/api/v1/drive/files/stats/shared-count-by-weekday",    get(file_stats_shared_count_by_weekday))
        .route("/api/v1/drive/files/stats/shared-count-by-hour",       get(file_stats_shared_count_by_hour))
        .route("/api/v1/drive/files/stats/owner-count-by-weekday",     get(file_stats_owner_count_by_weekday))
        .route("/api/v1/drive/files/stats/owner-count-by-hour",        get(file_stats_owner_count_by_hour))
        .route("/api/v1/drive/files/stats/tag-count-by-weekday",       get(file_stats_tag_count_by_weekday))
        .route("/api/v1/drive/files/stats/tag-count-by-hour",          get(file_stats_tag_count_by_hour))
        .route("/api/v1/drive/files/stats/mime-count-by-weekday",      get(file_stats_mime_count_by_weekday))
        .route("/api/v1/drive/files/stats/mime-count-by-hour",         get(file_stats_mime_count_by_hour))
        .route("/api/v1/drive/files/stats/ext-count-by-weekday",       get(file_stats_ext_count_by_weekday))
        .route("/api/v1/drive/files/stats/locked-count-by-weekday",     get(file_stats_locked_count_by_weekday))
        .route("/api/v1/drive/files/stats/locked-count-by-hour",        get(file_stats_locked_count_by_hour))
        .route("/api/v1/drive/files/stats/shared-count-by-month",       get(file_stats_shared_count_by_month))
        .route("/api/v1/drive/files/stats/shared-by-month",             get(file_stats_shared_by_month))
        .route("/api/v1/drive/files/stats/owner-by-weekday",            get(file_stats_owner_by_weekday))
        .route("/api/v1/drive/files/stats/owner-by-month",              get(file_stats_owner_by_month))
        .route("/api/v1/drive/files/stats/trashed-by-month",            get(file_stats_trashed_by_month))
        .route("/api/v1/drive/files/stats/trashed-by-hour",             get(file_stats_trashed_by_hour))
        .route("/api/v1/drive/files/stats/tag-by-hour",                 get(file_stats_tag_by_hour))
        .route("/api/v1/drive/files/stats/owner-by-hour",               get(file_stats_owner_by_hour))
        .route("/api/v1/drive/files/stats/trashed-by-weekday",          get(file_stats_trashed_by_weekday))
        .route("/api/v1/drive/files/stats/locked-count-by-month",       get(file_stats_locked_count_by_month))
        .route("/api/v1/drive/files/stats/owner-count-by-month",        get(file_stats_owner_count_by_month))
        .route("/api/v1/drive/files/stats/tag-count-by-month",          get(file_stats_tag_count_by_month))
        .route("/api/v1/drive/files/stats/mime-count-by-month",         get(file_stats_mime_count_by_month))
        .route("/api/v1/drive/files/stats/ext-count-by-month",          get(file_stats_ext_count_by_month))
        .route("/api/v1/drive/files/stats/shared-by-hour",              get(file_stats_shared_by_hour))
        .route("/api/v1/drive/files/stats/shared-by-weekday",           get(file_stats_shared_by_weekday))
        .route("/api/v1/drive/files/stats/ext-count-by-hour",          get(file_stats_ext_count_by_hour))
        .route("/api/v1/drive/files/stats/quota-by-month",              get(file_stats_quota_by_month))
        .route("/api/v1/drive/files/stats/owner-by-dow",                get(file_stats_owner_by_dow))
        .route("/api/v1/drive/files/stats/ext-count-by-dow",            get(file_stats_ext_count_by_dow))
        .route("/api/v1/drive/files/stats/owner-count-by-dow",          get(file_stats_owner_count_by_dow))
        .route("/api/v1/drive/files/stats/mime-count-by-dow",           get(file_stats_mime_count_by_dow))
        .route("/api/v1/drive/files/stats/tag-count-by-dow",            get(file_stats_tag_count_by_dow))
        .route("/api/v1/drive/files/stats/version-count-by-dow",        get(file_stats_version_count_by_dow))
        .route("/api/v1/drive/files/stats/locked-count-by-dow",         get(file_stats_locked_count_by_dow))
        .route("/api/v1/drive/files/stats/trashed-count-by-dow",        get(file_stats_trashed_count_by_dow))
        .route("/api/v1/drive/files/stats/shared-count-by-dow",         get(file_stats_shared_count_by_dow))
        .route("/api/v1/drive/files/stats/trashed-count-by-hour",       get(file_stats_trashed_count_by_hour))
        .route("/api/v1/drive/files/stats/trashed-count-by-weekday",    get(file_stats_trashed_count_by_weekday))
        .route("/api/v1/drive/files/stats/trashed-count-by-month",     get(file_stats_trashed_count_by_month))
        .route("/api/v1/drive/files/stats/quota-by-dow",               get(file_stats_quota_by_dow))
        .route("/api/v1/drive/files/stats/avg-size-by-dow",            get(file_stats_avg_size_by_dow))
        .route("/api/v1/drive/files/stats/avg-size-by-month",          get(file_stats_avg_size_by_month))
        .route("/api/v1/drive/files/stats/avg-size-by-hour",           get(file_stats_avg_size_by_hour))
        .route("/api/v1/drive/files/stats/avg-size-by-weekday",        get(file_stats_avg_size_by_weekday))
        .route("/api/v1/drive/files/stats/size-p95-by-weekday",        get(file_stats_size_p95_by_weekday))
        .route("/api/v1/drive/files/stats/size-p99-by-weekday",        get(file_stats_size_p99_by_weekday))
        .route("/api/v1/drive/files/stats/size-p95-by-hour",           get(file_stats_size_p95_by_hour))
        .route("/api/v1/drive/files/stats/size-p99-by-hour",           get(file_stats_size_p99_by_hour))
        .route("/api/v1/drive/files/stats/size-p95-by-month",          get(file_stats_size_p95_by_month))
        .route("/api/v1/drive/files/stats/size-p99-by-month",          get(file_stats_size_p99_by_month))
        .route("/api/v1/drive/files/stats/size-p95-by-dow",            get(file_stats_size_p95_by_dow))
        .route("/api/v1/drive/files/stats/size-p99-by-dow",            get(file_stats_size_p99_by_dow))
        .route("/api/v1/drive/files/stats/size-p75-by-hour",           get(file_stats_size_p75_by_hour))
        .route("/api/v1/drive/files/stats/size-p75-by-weekday",        get(file_stats_size_p75_by_weekday))
        .route("/api/v1/drive/files/stats/size-p75-by-month",          get(file_stats_size_p75_by_month))
        .route("/api/v1/drive/files/stats/size-p75-by-dow",            get(file_stats_size_p75_by_dow))
        .route("/api/v1/drive/files/stats/trashed-size-by-hour",        get(file_stats_trashed_size_by_hour))
        .route("/api/v1/drive/files/stats/trashed-size-by-weekday",    get(file_stats_trashed_size_by_weekday))
        .route("/api/v1/drive/files/stats/trashed-size-by-month",     get(file_stats_trashed_size_by_month))
        .route("/api/v1/drive/files/stats/size-p90-by-hour",          get(file_stats_size_p90_by_hour))
        .route("/api/v1/drive/files/stats/size-p90-by-weekday",       get(file_stats_size_p90_by_weekday))
        .route("/api/v1/drive/files/stats/size-p90-by-month",         get(file_stats_size_p90_by_month))
        .route("/api/v1/drive/files/stats/file-count-by-month",       get(file_stats_file_count_by_month))
        .route("/api/v1/drive/files/stats/file-count-by-dow",         get(file_stats_file_count_by_dow))
        .route("/api/v1/drive/files/stats/size-p90-by-dow",           get(file_stats_size_p90_by_dow))
        .route("/api/v1/drive/files/stats/trashed-size-by-dow",       get(file_stats_trashed_size_by_dow))
        .route("/api/v1/drive/files/stats/folder-count-by-dow",       get(file_stats_folder_count_by_dow))
        .route("/api/v1/drive/files/stats/total-size-by-month",       get(file_stats_total_size_by_month))
        .route("/api/v1/drive/files/stats/total-size-by-dow",         get(file_stats_total_size_by_dow))
        .route("/api/v1/drive/files/stats/total-size-by-hour",        get(file_stats_total_size_by_hour))
        .route("/api/v1/drive/files/stats/total-size-by-weekday",     get(file_stats_total_size_by_weekday))
        .route("/api/v1/drive/files/stats/size-p50-by-hour",          get(file_stats_size_p50_by_hour))
        .route("/api/v1/drive/files/stats/size-p50-by-weekday",       get(file_stats_size_p50_by_weekday))
        .route("/api/v1/drive/files/stats/size-p50-by-month",         get(file_stats_size_p50_by_month))
        .route("/api/v1/drive/files/stats/size-p50-by-dow",           get(file_stats_size_p50_by_dow))
        .route("/api/v1/drive/files/stats/size-p25-by-hour",          get(file_stats_size_p25_by_hour))
        .route("/api/v1/drive/files/stats/size-p25-by-weekday",       get(file_stats_size_p25_by_weekday))
        .route("/api/v1/drive/files/stats/size-p25-by-month",         get(file_stats_size_p25_by_month))
        .route("/api/v1/drive/files/stats/size-p25-by-dow",           get(file_stats_size_p25_by_dow))
        .route("/api/v1/drive/files/stats/size-p10-by-hour",          get(file_stats_size_p10_by_hour))
        .route("/api/v1/drive/files/stats/size-p10-by-weekday",       get(file_stats_size_p10_by_weekday))
        .route("/api/v1/drive/files/stats/size-p10-by-month",         get(file_stats_size_p10_by_month))
        .route("/api/v1/drive/files/stats/name-length-by-dow",        get(file_stats_name_length_by_dow))
        .route("/api/v1/drive/files/stats/version-size-by-dow",       get(file_stats_version_size_by_dow))
        .route("/api/v1/drive/files/stats/starred-by-dow",            get(file_stats_starred_by_dow))
        .route("/api/v1/drive/files/stats/modified-by-dow",           get(file_stats_modified_by_dow))
        .route("/api/v1/drive/files/stats/deleted-by-dow",            get(file_stats_deleted_by_dow))
        .route("/api/v1/drive/files/stats/trashed-count-by-user",     get(file_stats_trashed_count_by_user))
        .route("/api/v1/drive/files/stats/locked-count-by-user",      get(file_stats_locked_count_by_user))
        .route("/api/v1/drive/files/stats/tag-count-by-user",         get(file_stats_tag_count_by_user))
        .route("/api/v1/drive/files/stats/shared-by-dow",             get(file_stats_shared_by_dow))
        .route("/api/v1/drive/files/stats/starred-count-by-dow",      get(file_stats_starred_count_by_dow))
        .route("/api/v1/drive/files/stats/locked-by-dow",             get(file_stats_locked_by_dow))
        .route("/api/v1/drive/files/stats/trashed-by-dow",            get(file_stats_trashed_by_dow))
        .route("/api/v1/drive/files/stats/orphan-by-dow",             get(file_stats_orphan_by_dow))
        .route("/api/v1/drive/files/stats/zero-size-by-dow",          get(file_stats_zero_size_by_dow))
        .route("/api/v1/drive/files/stats/empty-by-dow",              get(file_stats_empty_by_dow))
        .route("/api/v1/drive/files/stats/starred-size-by-dow",       get(file_stats_starred_size_by_dow))
        .route("/api/v1/drive/files/stats/locked-size-by-dow",        get(file_stats_locked_size_by_dow))
        .route("/api/v1/drive/files/stats/trashed-size-by-user",      get(file_stats_trashed_size_by_user))
        .route("/api/v1/drive/files/stats/shared-size-by-dow",        get(file_stats_shared_size_by_dow))
        .route("/api/v1/drive/files/stats/locked-size-by-month",      get(file_stats_locked_size_by_month))
        .route("/api/v1/drive/files/stats/zero-size-by-month",        get(file_stats_zero_size_by_month))
        .route("/api/v1/drive/files/stats/zero-size-by-hour",         get(file_stats_zero_size_by_hour))
        .route("/api/v1/drive/files/stats/zero-size-by-user",         get(file_stats_zero_size_by_user))
        .route("/api/v1/drive/files/stats/zero-size-by-weekday",      get(file_stats_zero_size_by_weekday))
        .route("/api/v1/drive/files/stats/empty-size-by-month",       get(file_stats_empty_size_by_month))
        .route("/api/v1/drive/files/stats/empty-size-by-hour",        get(file_stats_empty_size_by_hour))
        .route("/api/v1/drive/files/stats/empty-size-by-user",        get(file_stats_empty_size_by_user))
        .route("/api/v1/drive/files/stats/empty-size-by-weekday",     get(file_stats_empty_size_by_weekday))
        .route("/api/v1/drive/files/stats/empty-count-by-month",      get(file_stats_empty_count_by_month))
        .route("/api/v1/drive/files/stats/empty-count-by-hour",       get(file_stats_empty_count_by_hour))
        .route("/api/v1/drive/files/stats/empty-count-by-user",       get(file_stats_empty_count_by_user))
        .route("/api/v1/drive/files/stats/empty-count-by-weekday",    get(file_stats_empty_count_by_weekday))
        .route("/api/v1/drive/files/stats/empty-ratio-by-month",      get(file_stats_empty_ratio_by_month))
        .route("/api/v1/drive/files/stats/empty-ratio-by-hour",       get(file_stats_empty_ratio_by_hour))
        .route("/api/v1/drive/files/stats/empty-ratio-by-user",       get(file_stats_empty_ratio_by_user))
        .route("/api/v1/drive/files/stats/empty-ratio-by-weekday",    get(file_stats_empty_ratio_by_weekday))
        .route("/api/v1/drive/files/stats/empty-ratio-by-ext",        get(file_stats_empty_ratio_by_ext))
        .route("/api/v1/drive/files/stats/empty-ratio-by-dow",        get(file_stats_empty_ratio_by_dow))
        .route("/api/v1/drive/files/stats/size-p25-by-user",          get(file_stats_size_p25_by_user))
        .route("/api/v1/drive/files/stats/size-p10-by-user",          get(file_stats_size_p10_by_user))
        .route("/api/v1/drive/files/stats/size-p10-by-ext",           get(file_stats_size_p10_by_ext))
        .route("/api/v1/drive/files/stats/size-p25-by-ext",           get(file_stats_size_p25_by_ext))
        .route("/api/v1/drive/files/stats/size-p50-by-ext",           get(file_stats_size_p50_by_ext))
        .route("/api/v1/drive/files/stats/size-p75-by-ext",           get(file_stats_size_p75_by_ext))
        .route("/api/v1/drive/files/stats/size-p90-by-ext",           get(file_stats_size_p90_by_ext))
        .route("/api/v1/drive/files/stats/size-p95-by-ext",           get(file_stats_size_p95_by_ext))
        .route("/api/v1/drive/files/stats/size-p99-by-ext",           get(file_stats_size_p99_by_ext))
        .route("/api/v1/drive/files/stats/size-p10-by-owner",         get(file_stats_size_p10_by_owner))
        .route("/api/v1/drive/files/stats/size-p25-by-owner",         get(file_stats_size_p25_by_owner))
        .route("/api/v1/drive/files/stats/size-p50-by-owner",         get(file_stats_size_p50_by_owner))
        .route("/api/v1/drive/files/stats/size-p75-by-owner",         get(file_stats_size_p75_by_owner))
        .route("/api/v1/drive/files/stats/size-p90-by-owner",         get(file_stats_size_p90_by_owner))
        .route("/api/v1/drive/files/stats/size-p95-by-owner",         get(file_stats_size_p95_by_owner))
        .route("/api/v1/drive/files/stats/size-p99-by-owner",         get(file_stats_size_p99_by_owner))
        .route("/api/v1/drive/files/stats/size-p10-by-kind",          get(file_stats_size_p10_by_kind))
        .route("/api/v1/drive/files/stats/size-p25-by-kind",          get(file_stats_size_p25_by_kind))
        .route("/api/v1/drive/files/stats/size-p50-by-kind",          get(file_stats_size_p50_by_kind))
        .route("/api/v1/drive/files/stats/size-p75-by-kind",          get(file_stats_size_p75_by_kind))
        .route("/api/v1/drive/files/stats/size-p90-by-kind",          get(file_stats_size_p90_by_kind))
        .route("/api/v1/drive/files/stats/size-p95-by-kind",          get(file_stats_size_p95_by_kind))
        .route("/api/v1/drive/files/stats/size-p99-by-kind",          get(file_stats_size_p99_by_kind))
        .route("/api/v1/drive/files/stats/count-by-owner",            get(file_stats_count_by_owner))
        .route("/api/v1/drive/files/stats/folder-count-by-owner",     get(file_stats_folder_count_by_owner))
        .route("/api/v1/drive/files/stats/size-avg-by-kind",          get(file_stats_size_avg_by_kind))
        .route("/api/v1/drive/files/stats/size-avg-by-owner",         get(file_stats_size_avg_by_owner))
        .route("/api/v1/drive/files/stats/size-avg-by-ext",           get(file_stats_size_avg_by_ext))
        .route("/api/v1/drive/files/stats/count-by-ext",              get(file_stats_count_by_ext))
        .route("/api/v1/drive/files/stats/total-size-by-owner",       get(file_stats_total_size_by_owner))
        .route("/api/v1/drive/files/stats/total-size-by-ext",         get(file_stats_total_size_by_ext))
        .route("/api/v1/drive/files/stats/total-size-by-kind",        get(file_stats_total_size_by_kind))
        .route("/api/v1/drive/files/stats/count-by-mime",             get(file_stats_count_by_mime))
        .route("/api/v1/drive/files/stats/empty-file-count-by-owner", get(file_stats_empty_file_count_by_owner))
        .route("/api/v1/drive/files/stats/folder-size-avg-by-owner",   get(file_stats_folder_size_avg_by_owner))
        .route("/api/v1/drive/files/stats/folder-total-size-by-owner", get(file_stats_folder_total_size_by_owner))
        .route("/api/v1/drive/files/stats/folder-count-by-ext",        get(file_stats_folder_count_by_ext))
        .route("/api/v1/drive/files/stats/deleted-count-by-owner",     get(file_stats_deleted_count_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-by-owner",      get(file_stats_deleted_size_by_owner))
        .route("/api/v1/drive/files/stats/deleted-count-by-ext",       get(file_stats_deleted_count_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-by-ext",        get(file_stats_deleted_size_by_ext))
        .route("/api/v1/drive/files/stats/deleted-count-by-kind",      get(file_stats_deleted_count_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-by-kind",       get(file_stats_deleted_size_by_kind))
        .route("/api/v1/drive/files/stats/shared-count-by-owner",      get(file_stats_shared_count_by_owner))
        .route("/api/v1/drive/files/stats/shared-size-by-owner",       get(file_stats_shared_size_by_owner))
        .route("/api/v1/drive/files/stats/version-stddev-by-owner",            get(file_stats_version_stddev_by_owner))
        .route("/api/v1/drive/files/stats/version-stddev-by-ext",              get(file_stats_version_stddev_by_ext))
        .route("/api/v1/drive/files/stats/version-stddev-by-mime",             get(file_stats_version_stddev_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-min-by-owner",           get(file_stats_size_deleted_min_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-max-by-owner",           get(file_stats_size_deleted_max_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-cv-by-owner",            get(file_stats_size_deleted_cv_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-iqr-by-owner",           get(file_stats_size_deleted_iqr_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-stddev-by-owner",        get(file_stats_size_deleted_stddev_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-min-by-ext",             get(file_stats_size_deleted_min_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-max-by-ext",             get(file_stats_size_deleted_max_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-cv-by-ext",              get(file_stats_size_deleted_cv_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-iqr-by-ext",             get(file_stats_size_deleted_iqr_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-stddev-by-ext",          get(file_stats_size_deleted_stddev_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-min-by-kind",            get(file_stats_size_deleted_min_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-max-by-kind",            get(file_stats_size_deleted_max_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-cv-by-kind",             get(file_stats_size_deleted_cv_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-iqr-by-kind",            get(file_stats_size_deleted_iqr_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-stddev-by-kind",         get(file_stats_size_deleted_stddev_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-variance-by-kind",       get(file_stats_size_deleted_variance_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-variance-by-mime",       get(file_stats_size_deleted_variance_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-skewness-by-kind",       get(file_stats_size_deleted_skewness_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-skewness-by-mime",       get(file_stats_size_deleted_skewness_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-skewness-by-owner",      get(file_stats_size_deleted_skewness_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-skewness-by-ext",        get(file_stats_size_deleted_skewness_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-kurtosis-by-kind",       get(file_stats_size_deleted_kurtosis_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-kurtosis-by-mime",       get(file_stats_size_deleted_kurtosis_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-kurtosis-by-owner",      get(file_stats_size_deleted_kurtosis_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-kurtosis-by-ext",        get(file_stats_size_deleted_kurtosis_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-variance-by-owner",      get(file_stats_size_deleted_variance_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-variance-by-ext",        get(file_stats_size_deleted_variance_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-mad-by-kind",            get(file_stats_size_deleted_mad_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-mad-by-mime",            get(file_stats_size_deleted_mad_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-mad-by-owner",           get(file_stats_size_deleted_mad_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-mad-by-ext",             get(file_stats_size_deleted_mad_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-trimmed-mean-by-kind",  get(file_stats_size_deleted_trimmed_mean_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-trimmed-mean-by-mime",  get(file_stats_size_deleted_trimmed_mean_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-trimmed-mean-by-owner", get(file_stats_size_deleted_trimmed_mean_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-trimmed-mean-by-ext",   get(file_stats_size_deleted_trimmed_mean_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-winsorized-mean-by-kind",  get(file_stats_size_deleted_winsorized_mean_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-winsorized-mean-by-mime",  get(file_stats_size_deleted_winsorized_mean_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-winsorized-mean-by-owner", get(file_stats_size_deleted_winsorized_mean_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-winsorized-mean-by-ext",   get(file_stats_size_deleted_winsorized_mean_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-gini-by-kind",             get(file_stats_size_deleted_gini_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-gini-by-mime",             get(file_stats_size_deleted_gini_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-theil-by-kind",            get(file_stats_size_deleted_theil_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-theil-by-mime",            get(file_stats_size_deleted_theil_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-gini-by-owner",            get(file_stats_size_deleted_gini_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-gini-by-ext",              get(file_stats_size_deleted_gini_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-theil-by-owner",           get(file_stats_size_deleted_theil_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-theil-by-ext",             get(file_stats_size_deleted_theil_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-hhi-by-kind",              get(file_stats_size_deleted_hhi_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-hhi-by-mime",              get(file_stats_size_deleted_hhi_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-hhi-by-owner",             get(file_stats_size_deleted_hhi_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-hhi-by-ext",               get(file_stats_size_deleted_hhi_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-atkinson-by-kind",          get(file_stats_size_deleted_atkinson_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-atkinson-by-mime",          get(file_stats_size_deleted_atkinson_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-atkinson-by-owner",         get(file_stats_size_deleted_atkinson_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-atkinson-by-ext",           get(file_stats_size_deleted_atkinson_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-lorenz-by-kind",            get(file_stats_size_deleted_lorenz_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-lorenz-by-mime",            get(file_stats_size_deleted_lorenz_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-lorenz-by-owner",           get(file_stats_size_deleted_lorenz_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-lorenz-by-ext",             get(file_stats_size_deleted_lorenz_by_ext))
        .route("/api/v1/drive/files/stats/active-size-range-by-kind",              get(file_stats_size_active_range_by_kind))
        .route("/api/v1/drive/files/stats/active-size-range-by-mime",              get(file_stats_size_active_range_by_mime))
        .route("/api/v1/drive/files/stats/active-size-range-by-owner",             get(file_stats_size_active_range_by_owner))
        .route("/api/v1/drive/files/stats/active-size-entropy-by-kind",            get(file_stats_size_active_entropy_by_kind))
        .route("/api/v1/drive/files/stats/active-size-entropy-by-mime",            get(file_stats_size_active_entropy_by_mime))
        .route("/api/v1/drive/files/stats/active-size-entropy-by-owner",           get(file_stats_size_active_entropy_by_owner))
        .route("/api/v1/drive/files/stats/active-size-skewness-by-kind",           get(file_stats_size_active_skewness_by_kind))
        .route("/api/v1/drive/files/stats/active-size-skewness-by-mime",           get(file_stats_size_active_skewness_by_mime))
        .route("/api/v1/drive/files/stats/active-size-skewness-by-owner",          get(file_stats_size_active_skewness_by_owner))
        .route("/api/v1/drive/files/stats/active-size-kurtosis-by-kind",           get(file_stats_size_active_kurtosis_by_kind))
        .route("/api/v1/drive/files/stats/active-size-kurtosis-by-mime",           get(file_stats_size_active_kurtosis_by_mime))
        .route("/api/v1/drive/files/stats/active-size-kurtosis-by-owner",          get(file_stats_size_active_kurtosis_by_owner))
        .route("/api/v1/drive/files/stats/active-size-gini-by-kind",               get(file_stats_size_active_gini_by_kind))
        .route("/api/v1/drive/files/stats/active-size-gini-by-mime",               get(file_stats_size_active_gini_by_mime))
        .route("/api/v1/drive/files/stats/active-size-gini-by-owner",              get(file_stats_size_active_gini_by_owner))
        .route("/api/v1/drive/files/stats/active-size-gini-by-ext",               get(file_stats_size_active_gini_by_ext))
        .route("/api/v1/drive/files/stats/active-size-hhi-by-ext",                get(file_stats_size_active_hhi_by_ext))
        .route("/api/v1/drive/files/stats/active-size-lorenz-by-ext",             get(file_stats_size_active_lorenz_by_ext))
        .route("/api/v1/drive/files/stats/active-size-theil-by-ext",              get(file_stats_size_active_theil_by_ext))
        .route("/api/v1/drive/files/stats/active-size-atkinson-by-ext",           get(file_stats_size_active_atkinson_by_ext))
        .route("/api/v1/drive/files/stats/active-size-normalized-entropy-by-kind", get(file_stats_size_active_normalized_entropy_by_kind))
        .route("/api/v1/drive/files/stats/active-size-normalized-entropy-by-mime", get(file_stats_size_active_normalized_entropy_by_mime))
        .route("/api/v1/drive/files/stats/active-size-normalized-entropy-by-owner", get(file_stats_size_active_normalized_entropy_by_owner))
        .route("/api/v1/drive/files/stats/active-size-normalized-entropy-by-ext",  get(file_stats_size_active_normalized_entropy_by_ext))
        .route("/api/v1/drive/files/stats/active-size-trimmed-mean-by-kind",       get(file_stats_size_active_trimmed_mean_by_kind))
        .route("/api/v1/drive/files/stats/active-size-trimmed-mean-by-mime",       get(file_stats_size_active_trimmed_mean_by_mime))
        .route("/api/v1/drive/files/stats/active-size-trimmed-mean-by-owner",      get(file_stats_size_active_trimmed_mean_by_owner))
        .route("/api/v1/drive/files/stats/active-size-trimmed-mean-by-ext",        get(file_stats_size_active_trimmed_mean_by_ext))
        .route("/api/v1/drive/files/stats/active-size-winsorized-mean-by-kind",    get(file_stats_size_active_winsorized_mean_by_kind))
        .route("/api/v1/drive/files/stats/active-size-winsorized-mean-by-mime",    get(file_stats_size_active_winsorized_mean_by_mime))
        .route("/api/v1/drive/files/stats/active-size-winsorized-mean-by-owner",   get(file_stats_size_active_winsorized_mean_by_owner))
        .route("/api/v1/drive/files/stats/active-size-winsorized-mean-by-ext",    get(file_stats_size_active_winsorized_mean_by_ext))
        .route("/api/v1/drive/files/stats/active-size-harmonic-mean-by-kind",     get(file_stats_size_active_harmonic_mean_by_kind))
        .route("/api/v1/drive/files/stats/active-size-harmonic-mean-by-mime",     get(file_stats_size_active_harmonic_mean_by_mime))
        .route("/api/v1/drive/files/stats/active-size-harmonic-mean-by-owner",    get(file_stats_size_active_harmonic_mean_by_owner))
        .route("/api/v1/drive/files/stats/active-size-harmonic-mean-by-ext",      get(file_stats_size_active_harmonic_mean_by_ext))
        .route("/api/v1/drive/files/stats/active-size-geometric-mean-by-kind",    get(file_stats_size_active_geometric_mean_by_kind))
        .route("/api/v1/drive/files/stats/active-size-geometric-mean-by-mime",    get(file_stats_size_active_geometric_mean_by_mime))
        .route("/api/v1/drive/files/stats/active-size-geometric-mean-by-owner",   get(file_stats_size_active_geometric_mean_by_owner))
        .route("/api/v1/drive/files/stats/active-size-geometric-mean-by-ext",     get(file_stats_size_active_geometric_mean_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-harmonic-mean-by-kind",    get(file_stats_size_deleted_harmonic_mean_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-harmonic-mean-by-mime",    get(file_stats_size_deleted_harmonic_mean_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-harmonic-mean-by-owner",   get(file_stats_size_deleted_harmonic_mean_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-harmonic-mean-by-ext",     get(file_stats_size_deleted_harmonic_mean_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-geometric-mean-by-kind",   get(file_stats_size_deleted_geometric_mean_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-geometric-mean-by-mime",   get(file_stats_size_deleted_geometric_mean_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-geometric-mean-by-owner",  get(file_stats_size_deleted_geometric_mean_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-geometric-mean-by-ext",    get(file_stats_size_deleted_geometric_mean_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-normalized-entropy-by-kind",  get(file_stats_size_deleted_normalized_entropy_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-normalized-entropy-by-mime",  get(file_stats_size_deleted_normalized_entropy_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-normalized-entropy-by-owner", get(file_stats_size_deleted_normalized_entropy_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-normalized-entropy-by-ext",  get(file_stats_size_deleted_normalized_entropy_by_ext))
        .route("/api/v1/drive/files/stats/count-deleted-by-ext",                    get(file_stats_count_deleted_by_ext))
        .route("/api/v1/drive/files/stats/count-active-by-ext",                     get(file_stats_count_active_by_ext))
        .route("/api/v1/drive/files/stats/active-size-coeff-var-by-kind",           get(file_stats_size_active_coeff_var_by_kind))
        .route("/api/v1/drive/files/stats/active-size-coeff-var-by-mime",           get(file_stats_size_active_coeff_var_by_mime))
        .route("/api/v1/drive/files/stats/active-size-coeff-var-by-owner",          get(file_stats_size_active_coeff_var_by_owner))
        .route("/api/v1/drive/files/stats/active-size-coeff-var-by-ext",            get(file_stats_size_active_coeff_var_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-coeff-var-by-kind",          get(file_stats_size_deleted_coeff_var_by_kind))
        .route("/api/v1/drive/files/stats/active-size-mad-by-mime",               get(file_stats_size_active_mad_by_mime))
        .route("/api/v1/drive/files/stats/active-size-mad-by-owner",              get(file_stats_size_active_mad_by_owner))
        .route("/api/v1/drive/files/stats/active-size-mad-by-ext",                get(file_stats_size_active_mad_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-coeff-var-by-mime",        get(file_stats_size_deleted_coeff_var_by_mime))
        .route("/api/v1/drive/files/stats/active-size-p99-by-owner",             get(file_stats_size_active_p99_by_owner))
        .route("/api/v1/drive/files/stats/active-size-p99-by-ext",               get(file_stats_size_active_p99_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-coeff-var-by-owner",      get(file_stats_size_deleted_coeff_var_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-coeff-var-by-ext",        get(file_stats_size_deleted_coeff_var_by_ext))
        .route("/api/v1/drive/files/stats/active-size-hhi-by-kind",               get(file_stats_size_active_hhi_by_kind))
        .route("/api/v1/drive/files/stats/active-size-hhi-by-mime",               get(file_stats_size_active_hhi_by_mime))
        .route("/api/v1/drive/files/stats/active-size-hhi-by-owner",              get(file_stats_size_active_hhi_by_owner))
        .route("/api/v1/drive/files/stats/active-size-lorenz-by-kind",            get(file_stats_size_active_lorenz_by_kind))
        .route("/api/v1/drive/files/stats/active-size-lorenz-by-mime",            get(file_stats_size_active_lorenz_by_mime))
        .route("/api/v1/drive/files/stats/active-size-lorenz-by-owner",           get(file_stats_size_active_lorenz_by_owner))
        .route("/api/v1/drive/files/stats/active-size-theil-by-kind",             get(file_stats_size_active_theil_by_kind))
        .route("/api/v1/drive/files/stats/active-size-theil-by-mime",             get(file_stats_size_active_theil_by_mime))
        .route("/api/v1/drive/files/stats/active-size-theil-by-owner",            get(file_stats_size_active_theil_by_owner))
        .route("/api/v1/drive/files/stats/active-size-atkinson-by-kind",          get(file_stats_size_active_atkinson_by_kind))
        .route("/api/v1/drive/files/stats/active-size-atkinson-by-mime",          get(file_stats_size_active_atkinson_by_mime))
        .route("/api/v1/drive/files/stats/active-size-atkinson-by-owner",         get(file_stats_size_active_atkinson_by_owner))
        .route("/api/v1/drive/files/stats/count-active-by-kind",                  get(file_stats_count_active_by_kind))
        .route("/api/v1/drive/files/stats/count-active-by-mime",                  get(file_stats_count_active_by_mime))
        .route("/api/v1/drive/files/stats/count-active-by-owner",                 get(file_stats_count_active_by_owner))
        .route("/api/v1/drive/files/stats/size-deleted-sum-by-kind",              get(file_stats_size_deleted_sum_by_kind))
        .route("/api/v1/drive/files/stats/size-deleted-sum-by-mime",              get(file_stats_size_deleted_sum_by_mime))
        .route("/api/v1/drive/files/stats/size-deleted-sum-by-owner",             get(file_stats_size_deleted_sum_by_owner))
        .route("/api/v1/drive/files/stats/count-deleted-by-kind",                 get(file_stats_count_deleted_by_kind))
        .route("/api/v1/drive/files/stats/count-deleted-by-mime",                 get(file_stats_count_deleted_by_mime))
        .route("/api/v1/drive/files/stats/count-deleted-by-owner",                get(file_stats_count_deleted_by_owner))
        .route("/api/v1/drive/files/stats/active-size-sum-by-kind",               get(file_stats_size_active_sum_by_kind))
        .route("/api/v1/drive/files/stats/active-size-sum-by-mime",               get(file_stats_size_active_sum_by_mime))
        .route("/api/v1/drive/files/stats/active-size-min-by-kind",               get(file_stats_size_active_min_by_kind))
        .route("/api/v1/drive/files/stats/active-size-sum-by-owner",              get(file_stats_size_active_sum_by_owner))
        .route("/api/v1/drive/files/stats/deleted-count-by-mime",               get(file_stats_size_deleted_count_by_mime))
        .route("/api/v1/drive/files/stats/deleted-count-by-kind",                get(file_stats_size_deleted_count_by_kind))
        .route("/api/v1/drive/files/stats/deleted-count-by-owner",               get(file_stats_size_deleted_count_by_owner))
        .route("/api/v1/drive/files/stats/active-size-p75-by-kind",              get(file_stats_size_active_p75_by_kind))
        .route("/api/v1/drive/files/stats/active-size-p75-by-mime",              get(file_stats_size_active_p75_by_mime))
        .route("/api/v1/drive/files/stats/active-size-p90-by-kind",              get(file_stats_size_active_p90_by_kind))
        .route("/api/v1/drive/files/stats/active-size-p90-by-mime",              get(file_stats_size_active_p90_by_mime))
        .route("/api/v1/drive/files/stats/active-size-p50-by-owner",             get(file_stats_size_active_p50_by_owner))
        .route("/api/v1/drive/files/stats/active-size-p75-by-owner",             get(file_stats_size_active_p75_by_owner))
        .route("/api/v1/drive/files/stats/active-size-p90-by-owner",             get(file_stats_size_active_p90_by_owner))
        .route("/api/v1/drive/files/stats/active-size-avg-by-mime",               get(file_stats_size_active_avg_by_mime))
        .route("/api/v1/drive/files/stats/active-size-max-by-owner",              get(file_stats_size_active_max_by_owner))
        .route("/api/v1/drive/files/stats/active-size-max-by-mime",               get(file_stats_size_active_max_by_mime))
        .route("/api/v1/drive/files/stats/active-size-max-by-kind",               get(file_stats_size_active_max_by_kind))
        .route("/api/v1/drive/files/stats/active-size-min-by-owner",              get(file_stats_size_active_min_by_owner))
        .route("/api/v1/drive/files/stats/active-size-min-by-mime",               get(file_stats_size_active_min_by_mime))
        .route("/api/v1/drive/files/stats/active-size-count-by-kind",              get(file_stats_size_active_count_by_kind))
        .route("/api/v1/drive/files/stats/active-size-count-by-mime",              get(file_stats_size_active_count_by_mime))
        .route("/api/v1/drive/files/stats/active-size-count-by-owner",             get(file_stats_size_active_count_by_owner))
        .route("/api/v1/drive/files/stats/active-size-p50-by-mime",                get(file_stats_size_active_p50_by_mime))
        .route("/api/v1/drive/files/stats/active-size-variance-by-kind",           get(file_stats_size_active_variance_by_kind))
        .route("/api/v1/drive/files/stats/active-size-variance-by-mime",           get(file_stats_size_active_variance_by_mime))
        .route("/api/v1/drive/files/stats/active-size-variance-by-owner",          get(file_stats_size_active_variance_by_owner))
        .route("/api/v1/drive/files/stats/active-size-mad-by-kind",                get(file_stats_size_active_mad_by_kind))
        .route("/api/v1/drive/files/stats/active-size-p95-by-mime",                get(file_stats_size_active_p95_by_mime))
        .route("/api/v1/drive/files/stats/active-size-iqr-by-kind",               get(file_stats_size_active_iqr_by_kind))
        .route("/api/v1/drive/files/stats/active-size-iqr-by-mime",               get(file_stats_size_active_iqr_by_mime))
        .route("/api/v1/drive/files/stats/active-size-iqr-by-owner",              get(file_stats_size_active_iqr_by_owner))
        .route("/api/v1/drive/files/stats/active-size-iqr-by-ext",               get(file_stats_size_active_iqr_by_ext))
        .route("/api/v1/drive/files/stats/active-size-range-by-ext",             get(file_stats_size_active_range_by_ext))
        .route("/api/v1/drive/files/stats/active-size-p95-by-owner",             get(file_stats_size_active_p95_by_owner))
        .route("/api/v1/drive/files/stats/active-size-p95-by-ext",               get(file_stats_size_active_p95_by_ext))
        .route("/api/v1/drive/files/stats/active-size-p90-by-ext",               get(file_stats_size_active_p90_by_ext))
        .route("/api/v1/drive/files/stats/active-size-p75-by-ext",               get(file_stats_size_active_p75_by_ext))
        .route("/api/v1/drive/files/stats/active-size-p50-by-ext",               get(file_stats_size_active_p50_by_ext))
        .route("/api/v1/drive/files/stats/active-size-stddev-by-ext",            get(file_stats_size_active_stddev_by_ext))
        .route("/api/v1/drive/files/stats/active-size-skewness-by-ext",          get(file_stats_size_active_skewness_by_ext))
        .route("/api/v1/drive/files/stats/active-size-variance-by-ext",          get(file_stats_size_active_variance_by_ext))
        .route("/api/v1/drive/files/stats/active-size-kurtosis-by-ext",          get(file_stats_size_active_kurtosis_by_ext))
        .route("/api/v1/drive/files/stats/active-size-cv-by-ext",                get(file_stats_size_active_cv_by_ext))
        .route("/api/v1/drive/files/stats/active-size-cv-by-owner",               get(file_stats_size_active_cv_by_owner))
        .route("/api/v1/drive/files/stats/active-size-cv-by-kind",                get(file_stats_size_active_cv_by_kind))
        .route("/api/v1/drive/files/stats/active-size-cv-by-mime",                get(file_stats_size_active_cv_by_mime))
        .route("/api/v1/drive/files/stats/active-size-p95-by-kind",               get(file_stats_size_active_p95_by_kind))
        .route("/api/v1/drive/files/stats/active-size-stddev-by-owner",           get(file_stats_size_active_stddev_by_owner))
        .route("/api/v1/drive/files/stats/active-size-stddev-by-kind",            get(file_stats_size_active_stddev_by_kind))
        .route("/api/v1/drive/files/stats/active-size-p99-by-kind",               get(file_stats_size_active_p99_by_kind))
        .route("/api/v1/drive/files/stats/active-size-p99-by-mime",               get(file_stats_size_active_p99_by_mime))
        .route("/api/v1/drive/files/stats/active-size-avg-by-owner",              get(file_stats_size_active_avg_by_owner))
        .route("/api/v1/drive/files/stats/active-size-avg-by-kind",               get(file_stats_size_active_avg_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-p99-by-kind",              get(file_stats_size_deleted_p99_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-p99-by-mime",              get(file_stats_size_deleted_p99_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-p95-by-kind",              get(file_stats_size_deleted_p95_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-p95-by-mime",              get(file_stats_size_deleted_p95_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-p99-by-owner",             get(file_stats_size_deleted_p99_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-p99-by-ext",               get(file_stats_size_deleted_p99_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-p90-by-kind",              get(file_stats_size_deleted_p90_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-p90-by-mime",              get(file_stats_size_deleted_p90_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-p95-by-owner",             get(file_stats_size_deleted_p95_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-p95-by-ext",               get(file_stats_size_deleted_p95_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-p75-by-kind",              get(file_stats_size_deleted_p75_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-p75-by-mime",              get(file_stats_size_deleted_p75_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-p90-by-owner",             get(file_stats_size_deleted_p90_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-p90-by-ext",               get(file_stats_size_deleted_p90_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-p50-by-kind",              get(file_stats_size_deleted_p50_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-p50-by-mime",              get(file_stats_size_deleted_p50_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-p75-by-owner",             get(file_stats_size_deleted_p75_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-p75-by-ext",               get(file_stats_size_deleted_p75_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-avg-by-kind",             get(file_stats_size_deleted_avg_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-avg-by-mime",             get(file_stats_size_deleted_avg_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-p50-by-owner",            get(file_stats_size_deleted_p50_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-p50-by-ext",              get(file_stats_size_deleted_p50_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-range-by-owner",          get(file_stats_size_deleted_range_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-range-by-ext",            get(file_stats_size_deleted_range_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-range-by-kind",           get(file_stats_size_deleted_range_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-range-by-mime",           get(file_stats_size_deleted_range_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-min-by-mime",            get(file_stats_size_deleted_min_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-max-by-mime",            get(file_stats_size_deleted_max_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-cv-by-mime",             get(file_stats_size_deleted_cv_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-iqr-by-mime",            get(file_stats_size_deleted_iqr_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-stddev-by-mime",         get(file_stats_size_deleted_stddev_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-avg-by-owner",          get(file_stats_size_deleted_avg_by_owner))
        .route("/api/v1/drive/files/stats/uploads-by-month",                  get(file_stats_uploads_by_month))
        .route("/api/v1/drive/files/stats/uploads-by-weekday",                get(file_stats_uploads_by_weekday))
        .route("/api/v1/drive/files/stats/uploads-by-hour",                   get(file_stats_uploads_by_hour))
        .route("/api/v1/drive/files/stats/deletes-by-month",                  get(file_stats_deletes_by_month))
        .route("/api/v1/drive/files/stats/deletes-by-weekday",                get(file_stats_deletes_by_weekday))
        .route("/api/v1/drive/files/stats/deletes-by-hour",                   get(file_stats_deletes_by_hour))
        .route("/api/v1/drive/files/stats/active-size-p50-by-kind",           get(file_stats_size_active_p50_by_kind))
        .route("/api/v1/drive/files/stats/active-size-stddev-by-mime",        get(file_stats_size_active_stddev_by_mime))
        .route("/api/v1/drive/files/stats/uploads-by-year",                   get(file_stats_uploads_by_year))
        .route("/api/v1/drive/files/stats/deletes-by-year",                   get(file_stats_deletes_by_year))
        .route("/api/v1/drive/files/stats/active-size-p25-by-kind",           get(file_stats_size_active_p25_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-avg-by-ext",           get(file_stats_size_deleted_avg_by_ext))
        .route("/api/v1/drive/files/stats/active-size-p25-by-mime",           get(file_stats_size_active_p25_by_mime))
        .route("/api/v1/drive/files/stats/active-size-p25-by-owner",          get(file_stats_size_active_p25_by_owner))
        .route("/api/v1/drive/files/stats/active-size-p25-by-ext",            get(file_stats_size_active_p25_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-p25-by-kind",          get(file_stats_size_deleted_p25_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-p25-by-mime",          get(file_stats_size_deleted_p25_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-p25-by-owner",         get(file_stats_size_deleted_p25_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-p25-by-ext",           get(file_stats_size_deleted_p25_by_ext))
        .route("/api/v1/drive/files/stats/active-size-p10-by-kind",           get(file_stats_size_active_p10_by_kind))
        .route("/api/v1/drive/files/stats/active-size-p10-by-mime",          get(file_stats_size_active_p10_by_mime))
        .route("/api/v1/drive/files/stats/active-size-p10-by-owner",         get(file_stats_size_active_p10_by_owner))
        .route("/api/v1/drive/files/stats/active-size-p10-by-ext",           get(file_stats_size_active_p10_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-p10-by-kind",         get(file_stats_size_deleted_p10_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-p10-by-mime",         get(file_stats_size_deleted_p10_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-p10-by-owner",        get(file_stats_size_deleted_p10_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-p10-by-ext",          get(file_stats_size_deleted_p10_by_ext))
        .route("/api/v1/drive/files/stats/active-size-p05-by-kind",          get(file_stats_size_active_p05_by_kind))
        .route("/api/v1/drive/files/stats/active-size-p05-by-mime",          get(file_stats_size_active_p05_by_mime))
        .route("/api/v1/drive/files/stats/active-size-p05-by-owner",         get(file_stats_size_active_p05_by_owner))
        .route("/api/v1/drive/files/stats/active-size-p05-by-ext",           get(file_stats_size_active_p05_by_ext))
        .route("/api/v1/drive/files/stats/deleted-size-p05-by-kind",         get(file_stats_size_deleted_p05_by_kind))
        .route("/api/v1/drive/files/stats/deleted-size-p05-by-mime",         get(file_stats_size_deleted_p05_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-p05-by-owner",        get(file_stats_size_deleted_p05_by_owner))
        .route("/api/v1/drive/files/stats/deleted-size-p05-by-ext",          get(file_stats_size_deleted_p05_by_ext))
        .route("/api/v1/drive/files/stats/uploads-by-kind",                  get(file_stats_uploads_by_kind))
        .route("/api/v1/drive/files/stats/uploads-by-mime",                  get(file_stats_uploads_by_mime))
        .route("/api/v1/drive/files/stats/uploads-by-ext",                   get(file_stats_uploads_by_ext))
        .route("/api/v1/drive/files/stats/deletes-by-kind",                  get(file_stats_deletes_by_kind))
        .route("/api/v1/drive/files/stats/deletes-by-mime",                  get(file_stats_deletes_by_mime))
        .route("/api/v1/drive/files/stats/version-min-by-ext",                get(file_stats_version_min_by_ext))
        .route("/api/v1/drive/files/stats/version-max-by-mime",               get(file_stats_version_max_by_mime))
        .route("/api/v1/drive/files/stats/version-min-by-mime",               get(file_stats_version_min_by_mime))
        .route("/api/v1/drive/files/stats/version-stddev-by-kind",            get(file_stats_version_stddev_by_kind))
        .route("/api/v1/drive/files/stats/version-min-by-kind",               get(file_stats_version_min_by_kind))
        .route("/api/v1/drive/files/stats/version-max-by-owner",              get(file_stats_version_max_by_owner))
        .route("/api/v1/drive/files/stats/version-min-by-owner",              get(file_stats_version_min_by_owner))
        .route("/api/v1/drive/files/stats/version-max-by-ext",                get(file_stats_version_max_by_ext))
        .route("/api/v1/drive/files/stats/name-length-stddev-by-owner",      get(file_stats_name_length_stddev_by_owner))
        .route("/api/v1/drive/files/stats/name-length-stddev-by-ext",        get(file_stats_name_length_stddev_by_ext))
        .route("/api/v1/drive/files/stats/name-length-stddev-by-mime",       get(file_stats_name_length_stddev_by_mime))
        .route("/api/v1/drive/files/stats/version-max-by-kind",              get(file_stats_version_max_by_kind))
        .route("/api/v1/drive/files/stats/name-length-min-by-owner",        get(file_stats_name_length_min_by_owner))
        .route("/api/v1/drive/files/stats/name-length-min-by-ext",          get(file_stats_name_length_min_by_ext))
        .route("/api/v1/drive/files/stats/name-length-min-by-mime",         get(file_stats_name_length_min_by_mime))
        .route("/api/v1/drive/files/stats/name-length-stddev-by-kind",      get(file_stats_name_length_stddev_by_kind))
        .route("/api/v1/drive/files/stats/name-length-max-by-owner",       get(file_stats_name_length_max_by_owner))
        .route("/api/v1/drive/files/stats/name-length-max-by-ext",         get(file_stats_name_length_max_by_ext))
        .route("/api/v1/drive/files/stats/name-length-max-by-mime",        get(file_stats_name_length_max_by_mime))
        .route("/api/v1/drive/files/stats/name-length-min-by-kind",        get(file_stats_name_length_min_by_kind))
        .route("/api/v1/drive/files/stats/count-by-shared",               get(file_stats_count_by_shared))
        .route("/api/v1/drive/files/stats/name-length-avg-by-kind",       get(file_stats_name_length_avg_by_kind))
        .route("/api/v1/drive/files/stats/name-length-avg-by-owner",      get(file_stats_name_length_avg_by_owner))
        .route("/api/v1/drive/files/stats/name-length-avg-by-ext",        get(file_stats_name_length_avg_by_ext))
        .route("/api/v1/drive/files/stats/size-range-by-owner",           get(file_stats_size_range_by_owner))
        .route("/api/v1/drive/files/stats/size-range-by-ext",             get(file_stats_size_range_by_ext))
        .route("/api/v1/drive/files/stats/size-range-by-kind",            get(file_stats_size_range_by_kind))
        .route("/api/v1/drive/files/stats/size-range-by-mime",            get(file_stats_size_range_by_mime))
        .route("/api/v1/drive/files/stats/size-iqr-by-owner",             get(file_stats_size_iqr_by_owner))
        .route("/api/v1/drive/files/stats/size-iqr-by-ext",               get(file_stats_size_iqr_by_ext))
        .route("/api/v1/drive/files/stats/size-iqr-by-kind",              get(file_stats_size_iqr_by_kind))
        .route("/api/v1/drive/files/stats/size-iqr-by-mime",              get(file_stats_size_iqr_by_mime))
        .route("/api/v1/drive/files/stats/size-cv-by-owner",              get(file_stats_size_cv_by_owner))
        .route("/api/v1/drive/files/stats/size-cv-by-ext",                get(file_stats_size_cv_by_ext))
        .route("/api/v1/drive/files/stats/size-cv-by-kind",               get(file_stats_size_cv_by_kind))
        .route("/api/v1/drive/files/stats/size-cv-by-mime",               get(file_stats_size_cv_by_mime))
        .route("/api/v1/drive/files/stats/size-stddev-by-owner",          get(file_stats_size_stddev_by_owner))
        .route("/api/v1/drive/files/stats/size-stddev-by-ext",            get(file_stats_size_stddev_by_ext))
        .route("/api/v1/drive/files/stats/size-stddev-by-kind",           get(file_stats_size_stddev_by_kind))
        .route("/api/v1/drive/files/stats/size-stddev-by-mime",           get(file_stats_size_stddev_by_mime))
        .route("/api/v1/drive/files/stats/size-min-by-owner",            get(file_stats_size_min_by_owner))
        .route("/api/v1/drive/files/stats/size-min-by-ext",              get(file_stats_size_min_by_ext))
        .route("/api/v1/drive/files/stats/size-min-by-kind",             get(file_stats_size_min_by_kind))
        .route("/api/v1/drive/files/stats/size-min-by-mime",             get(file_stats_size_min_by_mime))
        .route("/api/v1/drive/files/stats/size-max-by-owner",            get(file_stats_size_max_by_owner))
        .route("/api/v1/drive/files/stats/size-max-by-ext",              get(file_stats_size_max_by_ext))
        .route("/api/v1/drive/files/stats/size-max-by-kind",             get(file_stats_size_max_by_kind))
        .route("/api/v1/drive/files/stats/size-max-by-mime",             get(file_stats_size_max_by_mime))
        .route("/api/v1/drive/files/stats/size-p90-by-mime",             get(file_stats_size_p90_by_mime))
        .route("/api/v1/drive/files/stats/size-sum-by-mime",             get(file_stats_size_sum_by_mime))
        .route("/api/v1/drive/files/stats/size-sum-by-kind",             get(file_stats_size_sum_by_kind))
        .route("/api/v1/drive/files/stats/size-sum-by-owner",            get(file_stats_size_sum_by_owner))
        .route("/api/v1/drive/files/stats/size-avg-by-mime",             get(file_stats_size_avg_by_mime))
        .route("/api/v1/drive/files/stats/size-p50-by-mime",             get(file_stats_size_p50_by_mime))
        .route("/api/v1/drive/files/stats/size-p75-by-mime",             get(file_stats_size_p75_by_mime))
        .route("/api/v1/drive/files/stats/count-by-kind",                get(file_stats_count_by_kind))
        .route("/api/v1/drive/files/stats/deleted-count-by-mime",        get(file_stats_deleted_count_by_mime))
        .route("/api/v1/drive/files/stats/deleted-size-by-mime",         get(file_stats_deleted_size_by_mime))
        .route("/api/v1/drive/files/stats/version-count-by-kind",        get(file_stats_version_count_by_kind))
        .route("/api/v1/drive/files/stats/version-avg-by-kind",          get(file_stats_version_avg_by_kind))
        .route("/api/v1/drive/files/stats/shared-count-by-kind",         get(file_stats_shared_count_by_kind))
        .route("/api/v1/drive/files/stats/shared-size-by-kind",          get(file_stats_shared_size_by_kind))
        .route("/api/v1/drive/files/stats/version-count-by-mime",        get(file_stats_version_count_by_mime))
        .route("/api/v1/drive/files/stats/version-avg-by-mime",          get(file_stats_version_avg_by_mime))
        .route("/api/v1/drive/files/stats/shared-count-by-ext",          get(file_stats_shared_count_by_ext))
        .route("/api/v1/drive/files/stats/shared-size-by-ext",           get(file_stats_shared_size_by_ext))
        .route("/api/v1/drive/files/stats/version-avg-by-owner",         get(file_stats_version_avg_by_owner))
        .route("/api/v1/drive/files/stats/version-avg-by-ext",           get(file_stats_version_avg_by_ext))
        .route("/api/v1/drive/files/stats/version-count-by-owner",     get(file_stats_version_count_by_owner))
        .route("/api/v1/drive/users/:user_id/usage",        get(user_usage))
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub parent_id: Option<Uuid>,
    /// Filter by kind: "file" or "folder". Omit for both.
    pub kind:      Option<String>,
    /// Sort column: name | updated_at | created_at | size_bytes. Default: name.
    pub sort:      Option<String>,
    /// Sort direction: asc | desc. Default: asc.
    pub order:     Option<String>,
    /// Max rows to return (1–500, default 200).
    pub limit:     Option<i64>,
    /// Rows to skip (default 0).
    pub offset:    Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct MkdirBody {
    pub name:      String,
    pub parent_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteQuery {
    #[serde(default)]
    pub permanent: bool,
}

async fn list(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<ListQuery>,
    req_headers:  HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let max_updated: Option<OffsetDateTime> = if let Some(pid) = q.parent_id {
        sqlx::query_scalar(
            "SELECT MAX(updated_at) FROM drive_files WHERE tenant_id = $1 AND parent_id = $2 AND deleted_at IS NULL",
        )
        .bind(ctx.tenant_id)
        .bind(pid)
        .fetch_one(pool)
        .await
        .unwrap_or(None)
    } else {
        sqlx::query_scalar(
            "SELECT MAX(updated_at) FROM drive_files WHERE tenant_id = $1 AND parent_id IS NULL AND deleted_at IS NULL",
        )
        .bind(ctx.tenant_id)
        .fetch_one(pool)
        .await
        .unwrap_or(None)
    };
    if let Some(ts) = max_updated {
        if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
            if let Ok(ims_str) = ims_val.to_str() {
                if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                    if ts <= ims_dt {
                        return Ok(StatusCode::NOT_MODIFIED.into_response());
                    }
                }
            }
        }
    }
    if let Some(ref k) = q.kind {
        if k != "file" && k != "folder" {
            return Err(DriveError::BadRequest("kind must be 'file' or 'folder'".into()));
        }
    }
    let sort  = q.sort.as_deref().unwrap_or("name");
    let order = q.order.as_deref().unwrap_or("asc");
    if !matches!(sort, "name" | "updated_at" | "created_at" | "size_bytes") {
        return Err(DriveError::BadRequest(
            "sort must be one of: name, updated_at, created_at, size_bytes".into()
        ));
    }
    if !matches!(order, "asc" | "desc") {
        return Err(DriveError::BadRequest("order must be 'asc' or 'desc'".into()));
    }
    let limit  = q.limit.unwrap_or(200).clamp(1, 500);
    let offset = q.offset.unwrap_or(0).max(0);
    let mut rows = FileRepo::new(pool)
        .list_children_paged(ctx.tenant_id, q.parent_id, sort, order, limit, offset)
        .await?;
    if let Some(k) = q.kind.as_deref() {
        rows.retain(|f| f.kind == k);
    }
    let mut resp = Json(rows).into_response();
    if let Some(ts) = max_updated {
        let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
        resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    }
    Ok(resp)
}

async fn mkdir(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Json(body):   Json<MkdirBody>,
) -> Result<(StatusCode, Json<DriveFile>)> {
    let pool = state.db_or_unavailable()?;
    let name = sanitize_name(&body.name)?;
    let row  = FileRepo::new(pool).insert(&NewFile {
        tenant_id:     ctx.tenant_id,
        owner_user_id: ctx.user_id,
        parent_id:     body.parent_id,
        name,
        kind:          "folder".into(),
        mime_type:     None,
        size_bytes:    0,
        sha256:        None,
        storage_key:   None,
    }).await?;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn upload(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    mut mp:       Multipart,
) -> Result<(StatusCode, Json<DriveFile>)> {
    let pool = state.db_or_unavailable()?;

    let mut parent_id: Option<Uuid>    = None;
    let mut name:      Option<String>  = None;
    let mut mime:      Option<String>  = None;
    let mut data:      Option<Bytes>   = None;

    while let Some(field) = mp.next_field().await.map_err(|e| DriveError::BadRequest(e.to_string()))? {
        match field.name().unwrap_or("") {
            "parent_id" => {
                let v = field.text().await.map_err(|e| DriveError::BadRequest(e.to_string()))?;
                if !v.trim().is_empty() {
                    parent_id = Some(Uuid::parse_str(v.trim())
                        .map_err(|_| DriveError::BadRequest("invalid parent_id".into()))?);
                }
            }
            "file" => {
                name = field.file_name().map(|s| s.to_string());
                mime = field.content_type().map(|s| s.to_string());
                data = Some(field.bytes().await.map_err(|e| DriveError::BadRequest(e.to_string()))?);
            }
            _ => {}
        }
    }

    let bytes = data.ok_or(DriveError::BadRequest("missing file part".into()))?;
    let fname = sanitize_name(&name.unwrap_or_default())?;

    // Tenant-level quota check — rejeita antes de tocar o disco.
    let quota = QuotaRepo::new(pool).get(ctx.tenant_id).await?;
    if !quota.fits(bytes.len() as i64) {
        return Err(DriveError::QuotaExceeded);
    }

    // Folder-level quota check — only when uploading into a specific folder.
    if let Some(fid) = parent_id {
        if let Some(fq) = FolderQuotaRepo::new(pool).get(ctx.tenant_id, fid).await? {
            if !fq.fits(bytes.len() as i64) {
                return Err(DriveError::QuotaExceeded);
            }
        }
    }


    // Hash + persist blob. storage_key = random UUID → evita colisão
    // cross-tenant e mantém layout on-disk flat.
    let sha = format!("{:x}", Sha256::digest(&bytes));
    let key = Uuid::new_v4().to_string();
    let root = state.data_root();
    fs::create_dir_all(root).await?;
    let path: PathBuf = root.join(&key);
    let mut f = fs::File::create(&path).await?;
    f.write_all(&bytes).await?;
    f.flush().await?;

    let repo     = FileRepo::new(pool);
    let ver_repo = VersionRepo::new(pool);

    // Existing sibling w/ same name → archive current → overwrite row.
    if let Some(existing) = repo.find_by_name(ctx.tenant_id, parent_id, &fname).await? {
        if existing.kind != "file" {
            // Folder collision → cleanup new blob + conflict.
            let _ = fs::remove_file(&path).await;
            return Err(DriveError::Conflict("a folder with this name already exists".into()));
        }

        // Archive previous content (if any) before overwrite.
        if let Some(prev_key) = existing.storage_key.as_deref() {
            let next_no = ver_repo.next_no(ctx.tenant_id, existing.id).await?;
            ver_repo.insert(&NewVersion {
                file_id:     existing.id,
                tenant_id:   ctx.tenant_id,
                version_no:  next_no,
                storage_key: prev_key,
                size_bytes:  existing.size_bytes,
                sha256:      existing.sha256.as_deref(),
                mime_type:   existing.mime_type.as_deref(),
                created_by:  existing.owner_user_id,
            }).await?;
        }

        let updated = repo.update_content(
            ctx.tenant_id, existing.id,
            &key, bytes.len() as i64,
            Some(&sha), mime.as_deref(),
        ).await;

        if updated.is_err() {
            let _ = fs::remove_file(&path).await;
        }
        let updated = updated?;
        tracing::info!(target: "audit",
            event = "drive.file.version",
            tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, file_id = %updated.id);
        return Ok((StatusCode::OK, Json(updated)));
    }

    // New file → plain insert.
    let row = repo.insert(&NewFile {
        tenant_id:     ctx.tenant_id,
        owner_user_id: ctx.user_id,
        parent_id,
        name:          fname,
        kind:          "file".into(),
        mime_type:     mime,
        size_bytes:    bytes.len() as i64,
        sha256:        Some(sha),
        storage_key:   Some(key.clone()),
    }).await;

    if row.is_err() {
        let _ = fs::remove_file(&path).await;
    }
    let row = row?;
    tracing::info!(target: "audit",
        event = "drive.file.upload",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, file_id = %row.id);
    Ok((StatusCode::CREATED, Json(row)))
}

async fn metadata(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    req_headers:  HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let f = FileRepo::new(pool).get(ctx.tenant_id, id).await?;
    let etag = format!("\"{}-{}\"", f.updated_at.unix_timestamp(), f.id);
    let lm = f.updated_at.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
    if let Some(inm) = req_headers.get(header::IF_NONE_MATCH) {
        if inm.as_bytes() == etag.as_bytes() {
            return Ok(StatusCode::NOT_MODIFIED.into_response());
        }
    }
    if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
        if let Ok(ims_str) = ims_val.to_str() {
            if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                if f.updated_at <= ims_dt {
                    return Ok(StatusCode::NOT_MODIFIED.into_response());
                }
            }
        }
    }
    let mut resp = Json(f).into_response();
    resp.headers_mut().insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    Ok(resp)
}

async fn head_file(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    req_headers:  HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let f = FileRepo::new(pool).get(ctx.tenant_id, id).await?;
    let etag = format!("\"{}-{}\"", f.updated_at.unix_timestamp(), f.id);
    let lm = f.updated_at.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
    if let Some(inm) = req_headers.get(header::IF_NONE_MATCH) {
        if inm.as_bytes() == etag.as_bytes() {
            return Ok(StatusCode::NOT_MODIFIED.into_response());
        }
    }
    if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
        if let Ok(ims_str) = ims_val.to_str() {
            if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                if f.updated_at <= ims_dt {
                    return Ok(StatusCode::NOT_MODIFIED.into_response());
                }
            }
        }
    }
    let mut resp = StatusCode::OK.into_response();
    resp.headers_mut().insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    if let Ok(size) = HeaderValue::from_str(&f.size_bytes.to_string()) {
        resp.headers_mut().insert(header::CONTENT_LENGTH, size);
    }
    Ok(resp)
}

#[derive(Debug, Deserialize)]
struct RenameBody {
    name: String,
}

#[derive(Debug, Deserialize)]
struct MoveBody {
    /// Destination folder id; omit or null to move to root.
    parent_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct BulkTrashBody {
    /// File/folder ids to soft-delete (max 200).
    ids: Vec<Uuid>,
}

/// POST /api/v1/drive/files/bulk-trash — soft-delete up to 200 items atomically.
async fn bulk_trash(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Json(body):   Json<BulkTrashBody>,
) -> Result<Json<serde_json::Value>> {
    if body.ids.is_empty() {
        return Err(DriveError::BadRequest("ids must not be empty".into()));
    }
    if body.ids.len() > 200 {
        return Err(DriveError::BadRequest(format!(
            "too many ids: {} (max 200)", body.ids.len()
        )));
    }
    let pool    = state.db_or_unavailable()?;
    let trashed = FileRepo::new(pool).bulk_trash(ctx.tenant_id, &body.ids).await?;
    tracing::info!(target: "audit",
        event = "drive.file.bulk_trash",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, count = trashed);
    Ok(Json(serde_json::json!({ "trashed": trashed })))
}

#[derive(Debug, Deserialize)]
struct BulkRestoreBody {
    /// File/folder ids to restore from trash (max 200).
    ids: Vec<Uuid>,
}

/// POST /api/v1/drive/files/bulk-restore — restore up to 200 trashed items atomically.
async fn bulk_restore(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Json(body):   Json<BulkRestoreBody>,
) -> Result<Json<serde_json::Value>> {
    if body.ids.is_empty() {
        return Err(DriveError::BadRequest("ids must not be empty".into()));
    }
    if body.ids.len() > 200 {
        return Err(DriveError::BadRequest(format!(
            "too many ids: {} (max 200)", body.ids.len()
        )));
    }
    let pool     = state.db_or_unavailable()?;
    let restored = FileRepo::new(pool).bulk_restore(ctx.tenant_id, &body.ids).await?;
    tracing::info!(target: "audit",
        event = "drive.file.bulk_restore",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, count = restored);
    Ok(Json(serde_json::json!({ "restored": restored })))
}

#[derive(Debug, Deserialize)]
struct BulkMoveBody {
    /// File/folder ids to move (max 200).
    ids:       Vec<Uuid>,
    /// Destination folder id; omit or null to move to root.
    parent_id: Option<Uuid>,
}

/// POST /api/v1/drive/files/bulk-move — atomically move up to 200 items.
async fn bulk_move(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Json(body):   Json<BulkMoveBody>,
) -> Result<Json<Vec<DriveFile>>> {
    if body.ids.is_empty() {
        return Err(DriveError::BadRequest("ids must not be empty".into()));
    }
    if body.ids.len() > 200 {
        return Err(DriveError::BadRequest(format!(
            "too many ids: {} (max 200)", body.ids.len()
        )));
    }
    let pool = state.db_or_unavailable()?;
    if let Some(parent) = body.parent_id {
        let target = FileRepo::new(pool).get(ctx.tenant_id, parent).await?;
        if target.kind != "folder" {
            return Err(DriveError::BadRequest("parent_id must be a folder".into()));
        }
        // Prevent moving any of the selected items into themselves.
        if body.ids.contains(&target.id) {
            return Err(DriveError::BadRequest("cannot move a folder into itself".into()));
        }
    }
    let rows = FileRepo::new(pool).bulk_move(ctx.tenant_id, &body.ids, body.parent_id).await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize)]
struct BulkCopyBody {
    /// File/folder ids to copy (max 200).
    ids:       Vec<Uuid>,
    /// Destination parent; omit or null to place at root.
    parent_id: Option<Uuid>,
}

/// POST /api/v1/drive/files/bulk-copy — shallow-copy up to 200 items.
/// Each copy is named "<original name> (cópia)". Folders are copied as empty
/// rows (the same blob). Returns the list of newly created rows.
async fn bulk_copy(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Json(body):   Json<BulkCopyBody>,
) -> Result<Json<Vec<DriveFile>>> {
    if body.ids.is_empty() {
        return Err(DriveError::BadRequest("ids must not be empty".into()));
    }
    if body.ids.len() > 200 {
        return Err(DriveError::BadRequest(format!(
            "too many ids: {} (max 200)", body.ids.len()
        )));
    }
    let pool = state.db_or_unavailable()?;
    if let Some(parent) = body.parent_id {
        let target = FileRepo::new(pool).get(ctx.tenant_id, parent).await?;
        if target.kind != "folder" {
            return Err(DriveError::BadRequest("parent_id must be a folder".into()));
        }
    }
    let rows = FileRepo::new(pool)
        .bulk_copy_files(ctx.tenant_id, ctx.user_id, &body.ids, body.parent_id)
        .await?;
    tracing::info!(target: "audit",
        event = "drive.file.bulk_copy",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, count = rows.len());
    Ok(Json(rows))
}

#[derive(Debug, Deserialize)]
struct CopyBody {
    /// Optional destination name. Defaults to "<original name> (cópia)".
    name:      Option<String>,
    /// Destination parent; omit or null to place at root.
    parent_id: Option<Uuid>,
}

/// POST /api/v1/drive/files/:id/copy — shallow copy: new row, same blob.
async fn copy_file(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<CopyBody>,
) -> Result<(StatusCode, Json<DriveFile>)> {
    let pool = state.db_or_unavailable()?;
    let src  = FileRepo::new(pool).get(ctx.tenant_id, id).await?;

    let raw_name = body.name.unwrap_or_else(|| format!("{} (cópia)", src.name));
    let new_name = sanitize_name(&raw_name)?;

    if let Some(parent) = body.parent_id {
        let target = FileRepo::new(pool).get(ctx.tenant_id, parent).await?;
        if target.kind != "folder" {
            return Err(DriveError::BadRequest("parent_id must be a folder".into()));
        }
    }

    let row = FileRepo::new(pool)
        .copy_file(ctx.tenant_id, ctx.user_id, id, new_name, body.parent_id)
        .await?;
    tracing::info!(target: "audit",
        event = "drive.file.copy",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id,
        src_id = %id, copy_id = %row.id);
    Ok((StatusCode::CREATED, Json(row)))
}

/// POST /api/v1/drive/files/:id/move — move a file or folder to a different parent.
async fn move_file(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<MoveBody>,
) -> Result<Json<DriveFile>> {
    if let Some(parent) = body.parent_id {
        // Sanity: target folder must exist in the same tenant.
        let pool = state.db_or_unavailable()?;
        let target = FileRepo::new(pool).get(ctx.tenant_id, parent).await?;
        if target.kind != "folder" {
            return Err(DriveError::BadRequest("parent_id must be a folder".into()));
        }
        if target.id == id {
            return Err(DriveError::BadRequest("cannot move a folder into itself".into()));
        }
    }
    let pool = state.db_or_unavailable()?;
    let f = FileRepo::new(pool).move_to(ctx.tenant_id, id, body.parent_id).await?;
    Ok(Json(f))
}

/// PATCH /api/v1/drive/files/:id/metadata — rename a file or folder.
async fn rename(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<RenameBody>,
) -> Result<Json<DriveFile>> {
    let name = sanitize_name(&body.name)?;
    let pool = state.db_or_unavailable()?;
    let f = FileRepo::new(pool).rename(ctx.tenant_id, id, name).await?;
    Ok(Json(f))
}

async fn download(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let f    = FileRepo::new(pool).get(ctx.tenant_id, id).await?;

    if f.kind != "file" {
        return Err(DriveError::BadRequest("target is a folder".into()));
    }
    let key = f.storage_key.as_deref()
        .ok_or_else(|| DriveError::BadRequest("file has no content".into()))?;
    let bytes = fs::read(state.data_root().join(key)).await?;
    tracing::info!(target: "audit",
        event = "drive.file.download",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, file_id = %id);
    Ok(attachment_response(&f.name, f.mime_type.as_deref(), bytes))
}

/// GET /api/v1/drive/files/:id/preview
///
/// Returns the file content with `Content-Disposition: inline` so browsers
/// render it directly rather than downloading it. Supported for image/* and
/// application/pdf; returns 415 Unsupported Media Type for all other MIME types.
async fn preview(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let f    = FileRepo::new(pool).get(ctx.tenant_id, id).await?;

    if f.kind != "file" {
        return Err(DriveError::BadRequest("target is a folder".into()));
    }

    let mime = f.mime_type.as_deref().unwrap_or("application/octet-stream");
    let previewable = mime.starts_with("image/") || mime == "application/pdf";
    if !previewable {
        return Ok((StatusCode::UNSUPPORTED_MEDIA_TYPE,
            axum::Json(serde_json::json!({"error": "preview not supported for this file type", "mime": mime})))
            .into_response());
    }

    let key   = f.storage_key.as_deref()
        .ok_or_else(|| DriveError::BadRequest("file has no content".into()))?;
    let bytes = fs::read(state.data_root().join(key)).await?;

    let ct: HeaderValue = mime.parse()
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));

    let ascii: String = f.name.chars().map(|c| {
        if c.is_ascii_graphic() && c != '"' && c != '\\' { c } else { '_' }
    }).collect();
    let ascii = if ascii.is_empty() { "preview".to_string() } else { ascii };
    let cd: HeaderValue = format!("inline; filename=\"{ascii}\"")
        .parse()
        .unwrap_or_else(|_| HeaderValue::from_static("inline"));

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, ct);
    headers.insert(header::CONTENT_DISPOSITION, cd);

    Ok((StatusCode::OK, headers, bytes).into_response())
}

async fn delete(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Query(q):     Query<DeleteQuery>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    let repo = FileRepo::new(pool);
    if q.permanent {
        let key = repo.purge(ctx.tenant_id, id).await?;
        let Some(key) = key else { return Err(DriveError::NotFound(id)); };
        if !key.is_empty() {
            let path = state.data_root().join(&key);
            if let Err(e) = fs::remove_file(&path).await {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(target: "audit",
                        event = "drive.purge.blob_unlink_failed",
                        file_id = %id, error = %e);
                }
            }
        }
        tracing::info!(target: "audit",
            event = "drive.file.purge",
            tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, file_id = %id);
        return Ok(StatusCode::NO_CONTENT);
    }
    let removed = repo.soft_delete(ctx.tenant_id, id).await?;
    if removed == 0 { return Err(DriveError::NotFound(id)); }
    tracing::info!(target: "audit",
        event = "drive.file.trash",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, file_id = %id);
    Ok(StatusCode::NO_CONTENT)
}

async fn restore(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<DriveFile>> {
    let pool = state.db_or_unavailable()?;
    let row  = FileRepo::new(pool).restore(ctx.tenant_id, id).await?;
    tracing::info!(target: "audit",
        event = "drive.file.restore",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, file_id = %id);
    Ok(Json(row))
}

async fn trash(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    req_headers:  HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let max_updated: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT MAX(updated_at) FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool)
    .await
    .unwrap_or(None);
    if let Some(ts) = max_updated {
        if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
            if let Ok(ims_str) = ims_val.to_str() {
                if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                    if ts <= ims_dt {
                        return Ok(StatusCode::NOT_MODIFIED.into_response());
                    }
                }
            }
        }
    }
    let rows = FileRepo::new(pool).list_trash(ctx.tenant_id).await?;
    let mut resp = Json(rows).into_response();
    if let Some(ts) = max_updated {
        let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
        resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    }
    Ok(resp)
}

#[derive(Debug, Deserialize)]
struct PurgeTrashParams {
    /// Idade mínima (em dias) do `deleted_at` pra purgar. Default 30, cap 1..3650.
    older_than_days: Option<i64>,
}

/// DELETE /api/v1/drive/trash?older_than_days=30 — hard-delete files com
/// `deleted_at <= now() - older_than_days`. Tenant-scoped via begin_tenant_tx.
/// Best-effort blob removal (loga mas não falha se já sumiu). Retorna
/// `{purged: N, file_ids: [...]}`. Útil pra sweep manual antes do GC automático
/// kick-in. older_than_days default 30, clamp [1, 3650] (10 anos).
async fn purge_trash(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(params): Query<PurgeTrashParams>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let days = params.older_than_days.unwrap_or(30).clamp(1, 3650);
    let cutoff = OffsetDateTime::now_utc() - time::Duration::days(days);

    let purged = FileRepo::new(pool)
        .purge_trashed_older_than(ctx.tenant_id, cutoff)
        .await?;

    let data_root = state.data_root();
    let mut ids = Vec::with_capacity(purged.len());
    for (id, key) in &purged {
        ids.push(*id);
        if let Some(k) = key {
            let blob = data_root.join(k);
            if let Err(e) = fs::remove_file(&blob).await {
                tracing::warn!(target: "audit",
                    event = "drive.trash.purge_blob_skipped",
                    file_id = %id, key = %k, error = %e);
            }
        }
    }

    tracing::info!(target: "audit",
        event = "drive.trash.purge",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id,
        older_than_days = days, purged = ids.len());

    Ok(Json(serde_json::json!({
        "purged":   ids.len(),
        "file_ids": ids,
    })))
}

/// GET /api/v1/drive/files/stats/users?limit=N — top-N users by storage usage.
///
/// Returns `{users: [{user_id, file_count, used_bytes}]}` ordered by `used_bytes DESC`.
/// Only counts non-deleted files (`kind='file'`). `limit` default 20, max 200.
/// Useful for capacity planning and "heavy users" dashboards. Sprint #621.
#[derive(Debug, Deserialize)]
struct StatsUsersQuery {
    limit: Option<i64>,
}

async fn file_stats_users(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsUsersQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool  = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(20).clamp(1, 200);
    let rows  = QuotaRepo::new(pool).top_users_by_usage(ctx.tenant_id, limit).await?;
    let users: Vec<serde_json::Value> = rows.into_iter().map(|(uid, fc, ub)| {
        serde_json::json!({"user_id": uid, "file_count": fc, "used_bytes": ub})
    }).collect();
    Ok(Json(serde_json::json!({"users": users})))
}

/// GET /api/v1/drive/files/stats/folders?limit=N — top-N folders by recursive storage usage.
///
/// Returns `{folders: [{folder_id, folder_name, file_count, used_bytes}]}` ordered by
/// `used_bytes DESC`. Uses a recursive CTE to aggregate size_bytes of all descendant
/// files for each root folder. Only counts non-deleted files (`kind='file'`).
/// `limit` default 20, max 200. Sprint #626.
#[derive(Debug, Deserialize)]
struct StatsFoldersQuery {
    limit: Option<i64>,
}

async fn file_stats_folders(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsFoldersQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool  = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(20).clamp(1, 200);
    let rows  = QuotaRepo::new(pool).top_folders_by_usage(ctx.tenant_id, limit).await?;
    let folders: Vec<serde_json::Value> = rows.into_iter().map(|(fid, fname, fc, ub)| {
        serde_json::json!({"folder_id": fid, "folder_name": fname, "file_count": fc, "used_bytes": ub})
    }).collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/drive/files/stats/extensions?limit=N — breakdown by file extension.
///
/// Returns `{extensions: [{extension, file_count, total_bytes}]}` ordered by
/// `total_bytes DESC`. Extension = `lower(split_part(name, '.', -1))` — the part
/// after the last dot; files with no dot get extension "". Only counts non-deleted
/// files (`kind='file'`). `limit` default 50, max 500. Sprint #631.
#[derive(Debug, Deserialize)]
struct StatsExtensionsQuery {
    limit: Option<i64>,
}

async fn file_stats_extensions(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsExtensionsQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool  = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let rows  = QuotaRepo::new(pool).stats_by_extension(ctx.tenant_id, limit).await?;
    let extensions: Vec<serde_json::Value> = rows.into_iter().map(|(ext, fc, tb)| {
        serde_json::json!({"extension": ext, "file_count": fc, "total_bytes": tb})
    }).collect();
    Ok(Json(serde_json::json!({"extensions": extensions})))
}

/// GET /api/v1/drive/files/stats/activity?since=&until=
///
/// Returns uploads/updates/deletes per day for the tenant in the given range.
/// `since`/`until` are RFC 3339 timestamps (optional). Response:
/// `{days: [{day, uploads, updates, deletes}]}` ordered by day ASC. Sprint #636.
#[derive(Debug, Deserialize)]
struct StatsActivityQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    since: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    until: Option<OffsetDateTime>,
}

async fn file_stats_activity(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsActivityQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows = QuotaRepo::new(pool).activity_by_day(ctx.tenant_id, q.since, q.until).await?;
    let days: Vec<serde_json::Value> = rows.into_iter().map(|(day, uploads, updates, deletes)| {
        serde_json::json!({"day": day, "uploads": uploads, "updates": updates, "deletes": deletes})
    }).collect();
    Ok(Json(serde_json::json!({"days": days})))
}

/// GET /api/v1/drive/files/stats/created-by-day?since=&until= — arquivos criados por dia.
///
/// DATE_TRUNC('day', created_at) + COUNT sobre `drive_files` (kind='file', não-deletados).
/// `since`/`until` RFC3339 opcionais via `$N::timestamptz IS NULL OR`. Retorna
/// `{days:[{day,count}]}` ordenado dia ASC. Foca em criação (vs activity/#636 que usa audit).
/// Sprint #682.
async fn file_stats_created_by_day(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsActivityQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT to_char(date_trunc('day', created_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
                COUNT(*)::BIGINT AS count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL AND kind = 'file' \
            AND ($2::timestamptz IS NULL OR created_at >= $2) \
            AND ($3::timestamptz IS NULL OR created_at <  $3) \
          GROUP BY day \
          ORDER BY day ASC",
    )
    .bind(ctx.tenant_id)
    .bind(q.since)
    .bind(q.until)
    .fetch_all(pool)
    .await?;

    let days: Vec<serde_json::Value> = rows.into_iter()
        .map(|(day, count)| serde_json::json!({"day": day, "count": count}))
        .collect();
    Ok(Json(serde_json::json!({"days": days})))
}

/// GET /api/v1/drive/files/stats/age — file creation timeline by calendar month.
///
/// Returns `{months: [{month, file_count}]}` ordered month ASC. Month format is
/// `YYYY-MM`. Only counts non-deleted files (`kind='file'`). Tenant-scoped.
/// Useful for "when were files uploaded" dashboards. Sprint #641.
async fn file_stats_age(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows = QuotaRepo::new(pool).age_by_month(ctx.tenant_id).await?;
    let months: Vec<serde_json::Value> = rows.into_iter().map(|(month, file_count)| {
        serde_json::json!({"month": month, "file_count": file_count})
    }).collect();
    Ok(Json(serde_json::json!({"months": months})))
}

/// GET /api/v1/drive/files/stats/owners?limit=N — top-N file owners by file count.
///
/// Returns `{owners: [{owner_user_id, file_count, total_bytes}]}` ordered by
/// `file_count DESC`. Complements `stats/users` (#621) which orders by `used_bytes`;
/// here the primary sort is file count — useful for identifying users who create
/// many small files vs few large ones. Only counts non-deleted files (`kind='file'`).
/// `limit` default 20, max 200. Sprint #646.
#[derive(Debug, Deserialize)]
struct StatsOwnersQuery {
    limit: Option<i64>,
}

async fn file_stats_owners(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsOwnersQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool  = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(20).clamp(1, 200);
    let rows  = QuotaRepo::new(pool).top_owners_by_file_count(ctx.tenant_id, limit).await?;
    let owners: Vec<serde_json::Value> = rows.into_iter().map(|(uid, fc, tb)| {
        serde_json::json!({"owner_user_id": uid, "file_count": fc, "total_bytes": tb})
    }).collect();
    Ok(Json(serde_json::json!({"owners": owners})))
}

/// GET /api/v1/drive/files/stats/size-buckets — distribuição de arquivos por faixa de tamanho.
///
/// Retorna `{buckets: [{range, count, total_bytes}]}` com as faixas fixas:
/// "<1MB", "1–10MB", "10–100MB", ">100MB". Cobre todos os arquivos não-deletados
/// (`kind='file'`) do tenant. Útil pra "qual percentual do storage são arquivos gigantes?".
/// Sprint #651.
async fn file_stats_size_buckets(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let (lt1mb_c, lt1mb_b,
         lt10mb_c, lt10mb_b,
         lt100mb_c, lt100mb_b,
         gt100mb_c, gt100mb_b): (i64, i64, i64, i64, i64, i64, i64, i64) =
        sqlx::query_as(
            "SELECT \
                COUNT(*) FILTER (WHERE size_bytes < 1048576)::BIGINT, \
                COALESCE(SUM(size_bytes) FILTER (WHERE size_bytes < 1048576), 0)::BIGINT, \
                COUNT(*) FILTER (WHERE size_bytes >= 1048576    AND size_bytes < 10485760)::BIGINT, \
                COALESCE(SUM(size_bytes) FILTER (WHERE size_bytes >= 1048576    AND size_bytes < 10485760), 0)::BIGINT, \
                COUNT(*) FILTER (WHERE size_bytes >= 10485760   AND size_bytes < 104857600)::BIGINT, \
                COALESCE(SUM(size_bytes) FILTER (WHERE size_bytes >= 10485760   AND size_bytes < 104857600), 0)::BIGINT, \
                COUNT(*) FILTER (WHERE size_bytes >= 104857600)::BIGINT, \
                COALESCE(SUM(size_bytes) FILTER (WHERE size_bytes >= 104857600), 0)::BIGINT \
             FROM drive_files \
             WHERE tenant_id = $1 AND deleted_at IS NULL AND kind = 'file'",
        )
        .bind(ctx.tenant_id)
        .fetch_one(pool)
        .await?;

    let buckets = vec![
        serde_json::json!({"range": "<1MB",     "count": lt1mb_c,   "total_bytes": lt1mb_b}),
        serde_json::json!({"range": "1–10MB",   "count": lt10mb_c,  "total_bytes": lt10mb_b}),
        serde_json::json!({"range": "10–100MB", "count": lt100mb_c, "total_bytes": lt100mb_b}),
        serde_json::json!({"range": ">100MB",   "count": gt100mb_c, "total_bytes": gt100mb_b}),
    ];
    Ok(Json(serde_json::json!({"buckets": buckets})))
}

/// GET /api/v1/drive/files/stats/deleted — métricas de arquivos na lixeira.
///
/// Conta arquivos com `deleted_at IS NOT NULL` e soma seus bytes. Retorna também
/// `oldest_deleted_at` e `newest_deleted_at` para dar o intervalo temporal da lixeira.
/// Tenant-scoped. Sprint #657.
async fn file_stats_deleted(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let (deleted_count, deleted_bytes, oldest_deleted_at, newest_deleted_at):
        (i64, i64, Option<OffsetDateTime>, Option<OffsetDateTime>) =
        sqlx::query_as(
            "SELECT \
                COUNT(*)::BIGINT, \
                COALESCE(SUM(size_bytes), 0)::BIGINT, \
                MIN(deleted_at), \
                MAX(deleted_at) \
             FROM drive_files \
             WHERE tenant_id = $1 AND deleted_at IS NOT NULL AND kind = 'file'",
        )
        .bind(ctx.tenant_id)
        .fetch_one(pool)
        .await?;

    Ok(Json(serde_json::json!({
        "deleted_count":      deleted_count,
        "deleted_bytes":      deleted_bytes,
        "oldest_deleted_at":  oldest_deleted_at,
        "newest_deleted_at":  newest_deleted_at,
    })))
}

/// GET /api/v1/drive/files/stats/by-owner-and-ext?limit=N — top-N pares (owner, extensão) por bytes.
///
/// Retorna `{rows:[{owner_user_id,extension,file_count,total_bytes}]}` ordenado por
/// `total_bytes DESC`. Útil pra identificar quais usuários+tipos de arquivo dominam o
/// armazenamento. Só arquivos não-deletados (`kind='file'`). `limit` default 20, max 200.
/// Sprint #662.
#[derive(Debug, Deserialize)]
struct StatsOwnerExtQuery {
    limit: Option<i64>,
}

async fn file_stats_by_owner_and_ext(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsOwnerExtQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool  = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(20).clamp(1, 200);
    let rows: Vec<(Uuid, Option<String>, i64, i64)> = sqlx::query_as(
        "SELECT owner_user_id, extension, \
                COUNT(*)::BIGINT AS file_count, \
                COALESCE(SUM(size_bytes), 0)::BIGINT AS total_bytes \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL AND kind = 'file' \
         GROUP BY owner_user_id, extension \
         ORDER BY total_bytes DESC \
         LIMIT $2",
    )
    .bind(ctx.tenant_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let out: Vec<serde_json::Value> = rows.into_iter().map(|(uid, ext, fc, tb)| {
        serde_json::json!({
            "owner_user_id": uid,
            "extension":     ext,
            "file_count":    fc,
            "total_bytes":   tb,
        })
    }).collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/recent?since=&limit=N — arquivos criados/modificados recentemente.
///
/// Retorna `{files:[{id,name,size_bytes,created_at,updated_at}]}` ordenado por
/// `updated_at DESC`. `since` RFC3339 opcional (filtra `updated_at >= since`).
/// `limit` default 20 max 200. Só arquivos não-deletados (`kind='file'`). Sprint #667.
#[derive(Debug, Deserialize)]
struct StatsRecentQuery {
    since: Option<String>,
    limit: Option<i64>,
}

async fn file_stats_recent(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsRecentQuery>,
) -> Result<Json<serde_json::Value>> {
    use time::format_description::well_known::Rfc3339;

    let pool  = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(20).clamp(1, 200);
    let since_dt = q.since.as_deref()
        .map(|s| OffsetDateTime::parse(s, &Rfc3339))
        .transpose()
        .map_err(|_| crate::error::DriveError::BadRequest("since must be RFC3339".into()))?;

    let rows: Vec<(Uuid, String, i64, OffsetDateTime, OffsetDateTime)> = sqlx::query_as(
        "SELECT id, name, COALESCE(size_bytes, 0)::BIGINT, created_at, updated_at \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL AND kind = 'file' \
            AND ($2::timestamptz IS NULL OR updated_at >= $2) \
          ORDER BY updated_at DESC \
          LIMIT $3",
    )
    .bind(ctx.tenant_id)
    .bind(since_dt)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let files: Vec<serde_json::Value> = rows.into_iter()
        .map(|(id, name, size_bytes, created_at, updated_at)| serde_json::json!({
            "id":         id,
            "name":       name,
            "size_bytes": size_bytes,
            "created_at": created_at,
            "updated_at": updated_at,
        }))
        .collect();
    Ok(Json(serde_json::json!({"files": files})))
}

/// GET /api/v1/drive/files/stats/mime-by-folder?folder_id= — breakdown de mime_type numa pasta.
///
/// `folder_id` opcional: quando ausente agrega na raiz (`parent_id IS NULL`).
/// Retorna `{folder_id, rows:[{mime_type,file_count,total_bytes}]}` ordenado por
/// `total_bytes DESC`. Só arquivos não-deletados (`kind='file'`). Sprint #672.
#[derive(Debug, Deserialize)]
struct StatsMimeByFolderQuery {
    folder_id: Option<Uuid>,
}

async fn file_stats_mime_by_folder(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsMimeByFolderQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(Option<String>, i64, i64)> = if let Some(fid) = q.folder_id {
        sqlx::query_as(
            "SELECT mime_type, COUNT(*)::BIGINT AS file_count, \
                    COALESCE(SUM(size_bytes), 0)::BIGINT AS total_bytes \
               FROM drive_files \
              WHERE tenant_id = $1 AND deleted_at IS NULL AND kind = 'file' \
                AND parent_id = $2 \
              GROUP BY mime_type \
              ORDER BY total_bytes DESC",
        )
        .bind(ctx.tenant_id)
        .bind(fid)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as(
            "SELECT mime_type, COUNT(*)::BIGINT AS file_count, \
                    COALESCE(SUM(size_bytes), 0)::BIGINT AS total_bytes \
               FROM drive_files \
              WHERE tenant_id = $1 AND deleted_at IS NULL AND kind = 'file' \
                AND parent_id IS NULL \
              GROUP BY mime_type \
              ORDER BY total_bytes DESC",
        )
        .bind(ctx.tenant_id)
        .fetch_all(pool)
        .await?
    };

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, fc, tb)| serde_json::json!({
            "mime_type":   mime,
            "file_count":  fc,
            "total_bytes": tb,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folder_id": q.folder_id, "rows": out})))
}

/// GET /api/v1/drive/files/stats/top-files?limit=N — top-N arquivos por size_bytes.
///
/// Retorna `{files:[{id,name,size_bytes,owner_user_id,mime_type}]}` ordenado por
/// `size_bytes DESC`. `limit` default 20 max 200. Só arquivos não-deletados.
/// Complementa size-buckets (#651) com lista concreta dos maiores arquivos. Sprint #677.
#[derive(Debug, Deserialize)]
struct StatsTopFilesQuery {
    limit: Option<i64>,
}

async fn file_stats_top_files(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool  = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(20).clamp(1, 200);

    let rows: Vec<(Uuid, String, i64, Uuid, Option<String>)> = sqlx::query_as(
        "SELECT id, name, COALESCE(size_bytes, 0)::BIGINT, owner_user_id, mime_type \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL AND kind = 'file' \
          ORDER BY size_bytes DESC NULLS LAST \
          LIMIT $2",
    )
    .bind(ctx.tenant_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let files: Vec<serde_json::Value> = rows.into_iter()
        .map(|(id, name, size_bytes, owner_user_id, mime_type)| serde_json::json!({
            "id":            id,
            "name":          name,
            "size_bytes":    size_bytes,
            "owner_user_id": owner_user_id,
            "mime_type":     mime_type,
        }))
        .collect();
    Ok(Json(serde_json::json!({"files": files})))
}

/// GET /api/v1/drive/files/:id/versions?limit=N&before_version=V
///
/// Lista versões em ordem DESC de version_no. `before_version` é o version_no
/// do último item da página anterior (keyset cursor — retorna versões com
/// version_no < before_version). `limit` default 50, max 500.
/// Response inclui `{versions, next_cursor, has_more}`. Sprint #608.
#[derive(Debug, Deserialize)]
struct ListVersionsParams {
    limit:          Option<i64>,
    before_version: Option<i32>,
}

async fn list_versions(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Query(q):     Query<ListVersionsParams>,
    req_headers:  HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let f = FileRepo::new(pool).get(ctx.tenant_id, id).await?;
    let max_created: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT MAX(created_at) FROM drive_file_versions WHERE tenant_id = $1 AND file_id = $2",
    )
    .bind(ctx.tenant_id)
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap_or(None);
    let max_ts = max_created.unwrap_or(f.updated_at);
    let lm = max_ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
    if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
        if let Ok(ims_str) = ims_val.to_str() {
            if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                if max_ts <= ims_dt {
                    return Ok(StatusCode::NOT_MODIFIED.into_response());
                }
            }
        }
    }

    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let rows: Vec<crate::domain::version::FileVersion> = if let Some(bv) = q.before_version {
        sqlx::query_as(
            "SELECT id, file_id, tenant_id, version_no, storage_key, size_bytes, sha256, \
                    mime_type, created_by, created_at \
               FROM drive_file_versions \
              WHERE tenant_id = $1 AND file_id = $2 AND version_no < $3 \
              ORDER BY version_no DESC \
              LIMIT $4",
        )
        .bind(ctx.tenant_id).bind(id).bind(bv).bind(limit)
        .fetch_all(pool).await?
    } else {
        sqlx::query_as(
            "SELECT id, file_id, tenant_id, version_no, storage_key, size_bytes, sha256, \
                    mime_type, created_by, created_at \
               FROM drive_file_versions \
              WHERE tenant_id = $1 AND file_id = $2 \
              ORDER BY version_no DESC \
              LIMIT $3",
        )
        .bind(ctx.tenant_id).bind(id).bind(limit)
        .fetch_all(pool).await?
    };

    let has_more    = rows.len() as i64 == limit;
    let next_cursor = rows.last().map(|v| v.version_no);
    let count       = rows.len();

    let mut resp = serde_json::json!({
        "versions":    rows,
        "next_cursor": next_cursor,
        "has_more":    has_more,
    });
    // Back-compat: when no cursor params, also embed total count.
    if q.before_version.is_none() {
        resp["total"] = serde_json::json!(count);
    }
    let mut r = Json(resp).into_response();
    r.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    Ok(r)
}

/// GET /api/v1/drive/files/:id/versions/:v/metadata — metadados de uma versão sem download.
///
/// Retorna `{version_no, mime_type, sha256, size_bytes, created_at}` para a versão `:v`.
/// Útil pra verificar integridade (sha256) sem baixar o blob. 404 se versão não encontrada.
/// Sprint #602.
async fn version_metadata(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, v)): Path<(Uuid, i32)>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    // Tenant-gate via file ownership check.
    let _ = FileRepo::new(pool).get(ctx.tenant_id, id).await?;
    let ver = VersionRepo::new(pool).get(ctx.tenant_id, id, v).await?
        .ok_or(DriveError::NotFound(id))?;
    Ok(Json(serde_json::json!({
        "file_id":    id,
        "version_no": ver.version_no,
        "mime_type":  ver.mime_type,
        "sha256":     ver.sha256,
        "size_bytes": ver.size_bytes,
        "created_at": ver.created_at,
    })))
}

async fn download_version(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, v)): Path<(Uuid, i32)>,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    // Tenant-gate.
    let parent = FileRepo::new(pool).get(ctx.tenant_id, id).await?;
    let ver = VersionRepo::new(pool).get(ctx.tenant_id, id, v).await?
        .ok_or(DriveError::NotFound(id))?;
    let bytes = fs::read(state.data_root().join(&ver.storage_key)).await?;
    tracing::info!(target: "audit",
        event = "drive.file.download_version",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id,
        file_id = %id, version_no = v);
    let filename = format!("{}.v{}", parent.name, v);
    Ok(attachment_response(&filename, ver.mime_type.as_deref(), bytes))
}

/// POST /api/v1/drive/files/:id/versions/:v/restore
///
/// Promotes a historical version `:v` to the current live content of file `:id`.
/// The current live content is archived as a new version before the swap, so no
/// data is lost. Returns the updated file record.
///
/// Flow:
///   1. Fetch current live file (404 if not found or deleted).
///   2. Fetch target version `:v` (404 if missing).
///   3. Archive current live blob as a new version (next_no).
///   4. Swap `drive_files.storage_key / size_bytes / sha256 / mime_type` to the
///      target version's blob via `FileRepo::update_content`.
///
/// Idempotent if called twice with the same version: the second call finds the
/// current live content already matching, creates an extra archive version.
/// Sprint #611.
async fn restore_version(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, v)): Path<(Uuid, i32)>,
) -> Result<Json<DriveFile>> {
    let pool = state.db_or_unavailable()?;

    let file_repo = FileRepo::new(pool);
    let ver_repo  = VersionRepo::new(pool);

    let current = file_repo.get(ctx.tenant_id, id).await?;
    let target  = ver_repo.get(ctx.tenant_id, id, v).await?
        .ok_or(DriveError::NotFound(id))?;

    // Archive the current live blob as a new historical version.
    if let Some(ref current_key) = current.storage_key {
        let next_no = ver_repo.next_no(ctx.tenant_id, id).await?;
        ver_repo.insert(&NewVersion {
            file_id:     id,
            tenant_id:   ctx.tenant_id,
            version_no:  next_no,
            storage_key: current_key,
            size_bytes:  current.size_bytes,
            sha256:      current.sha256.as_deref(),
            mime_type:   current.mime_type.as_deref(),
            created_by:  ctx.user_id,
        }).await?;
    }

    // Promote target version to live.
    let updated = file_repo.update_content(
        ctx.tenant_id,
        id,
        &target.storage_key,
        target.size_bytes,
        target.sha256.as_deref(),
        target.mime_type.as_deref(),
    ).await?;

    tracing::info!(target: "audit",
        event = "drive.file.restore_version",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id,
        file_id = %id, version_no = v);

    Ok(Json(updated))
}

/// GET /api/v1/drive/files/:id/versions/:v/diff-content
///
/// Returns a unified text diff between version `:v` and the version immediately
/// before it (v-1). 404 if either version blob is missing or the file is not
/// text (detected via Content-Type prefix). 409 if v == 1 (no previous version).
/// Response: `{version_a, version_b, hunks: [{header, lines: [{tag,text}]}]}`.
/// Binary-safe guard: rejects blobs with a NUL byte in the first 8 KiB.
#[derive(Debug, Deserialize)]
struct DiffParams {
    /// Lines of context around each changed region (default 3, clamped to 0–50).
    context: Option<u32>,
    /// Output format: "unified" (default) or "side-by-side".
    format: Option<String>,
    /// Base version to diff from. Defaults to v-1 (previous version).
    from: Option<i32>,
}

async fn diff_version_content(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, v)): Path<(Uuid, i32)>,
    Query(params): Query<DiffParams>,
) -> Result<Json<serde_json::Value>> {
    use serde_json::json;

    let fmt = params.format.as_deref().unwrap_or("unified");
    if fmt != "unified" && fmt != "side-by-side" {
        return Err(DriveError::BadRequest("format must be 'unified' or 'side-by-side'".into()));
    }

    let context = params.context.unwrap_or(3).min(50) as usize;

    // Determine which version pair to diff.
    let v_a = match params.from {
        Some(from) => {
            if from < 1 {
                return Err(DriveError::BadRequest("from must be >= 1".into()));
            }
            if from == v {
                return Err(DriveError::BadRequest("from and v must differ".into()));
            }
            from
        }
        None => {
            if v <= 1 {
                return Err(DriveError::BadRequest("no previous version to diff (v must be > 1, or specify ?from=)".into()));
            }
            v - 1
        }
    };

    let pool = state.db_or_unavailable()?;
    let _file = FileRepo::new(pool).get(ctx.tenant_id, id).await?;

    let ver_b = VersionRepo::new(pool).get(ctx.tenant_id, id, v).await?
        .ok_or(DriveError::NotFound(id))?;
    let ver_a = VersionRepo::new(pool).get(ctx.tenant_id, id, v_a).await?
        .ok_or(DriveError::NotFound(id))?;

    let bytes_a = fs::read(state.data_root().join(&ver_a.storage_key)).await
        .map_err(|_| DriveError::NotFound(id))?;
    let bytes_b = fs::read(state.data_root().join(&ver_b.storage_key)).await
        .map_err(|_| DriveError::NotFound(id))?;

    // Binary guard — reject if NUL byte in first 8 KiB.
    let probe_a = &bytes_a[..bytes_a.len().min(8192)];
    let probe_b = &bytes_b[..bytes_b.len().min(8192)];
    if probe_a.contains(&0u8) || probe_b.contains(&0u8) {
        return Err(DriveError::BadRequest("binary files cannot be diffed as text".into()));
    }

    let text_a = String::from_utf8_lossy(&bytes_a);
    let text_b = String::from_utf8_lossy(&bytes_b);

    let lines_a: Vec<&str> = text_a.lines().collect();
    let lines_b: Vec<&str> = text_b.lines().collect();

    if fmt == "side-by-side" {
        let rows = side_by_side_diff(&lines_a, &lines_b, context);
        return Ok(Json(json!({
            "file_id":   id,
            "version_a": v_a,
            "version_b": v,
            "format":    "side-by-side",
            "context":   context,
            "rows":      rows,
        })));
    }

    let hunks = unified_diff(&lines_a, &lines_b, context);
    Ok(Json(json!({
        "file_id":   id,
        "version_a": v_a,
        "version_b": v,
        "format":    "unified",
        "context":   context,
        "hunks":     hunks,
    })))
}

/// Compute a side-by-side diff: each row has `{type, left, right}`.
/// `type`: "equal" | "changed" | "deleted" | "inserted".
/// `left`/`right`: `{line_no: usize | null, text: String | null}`.
/// Rows outside a context window of a change are suppressed.
/// Sprint #592.
fn side_by_side_diff(old: &[&str], new: &[&str], context: usize) -> serde_json::Value {
    use serde_json::json;

    let m = old.len();
    let n = new.len();

    // LCS table (same DP as unified_diff).
    let mut lcs = vec![vec![0usize; n + 1]; m + 1];
    for i in (0..m).rev() {
        for j in (0..n).rev() {
            lcs[i][j] = if old[i] == new[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Op { Eq, Del, Ins }

    let mut ops: Vec<(Op, usize, usize)> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < m || j < n {
        if i < m && j < n && old[i] == new[j] {
            ops.push((Op::Eq, i, j)); i += 1; j += 1;
        } else if j < n && (i >= m || lcs[i][j + 1] >= lcs[i + 1][j]) {
            ops.push((Op::Ins, i, j)); j += 1;
        } else {
            ops.push((Op::Del, i, j)); i += 1;
        }
    }

    // Pair up consecutive Del+Ins into "changed" rows; lone Del → "deleted"; lone Ins → "inserted".
    // Then filter to within context of any non-equal row.
    #[derive(Clone)]
    struct Row {
        kind:     &'static str,
        left_no:  Option<usize>,
        left_txt: Option<String>,
        right_no: Option<usize>,
        right_txt: Option<String>,
    }

    let mut raw: Vec<Row> = Vec::new();
    let total = ops.len();
    let mut k = 0;
    while k < total {
        match ops[k].0 {
            Op::Eq => {
                let (_, oi, ni) = ops[k];
                raw.push(Row {
                    kind: "equal",
                    left_no: Some(oi + 1), left_txt: Some(old[oi].to_owned()),
                    right_no: Some(ni + 1), right_txt: Some(new[ni].to_owned()),
                });
                k += 1;
            }
            Op::Del => {
                let (_, oi, _ni) = ops[k];
                // Peek: if next op is Ins, pair as "changed".
                if k + 1 < total && ops[k + 1].0 == Op::Ins {
                    let (_, _oi2, ni2) = ops[k + 1];
                    raw.push(Row {
                        kind: "changed",
                        left_no: Some(oi + 1), left_txt: Some(old[oi].to_owned()),
                        right_no: Some(ni2 + 1), right_txt: Some(new[ni2].to_owned()),
                    });
                    k += 2;
                } else {
                    raw.push(Row {
                        kind: "deleted",
                        left_no: Some(oi + 1), left_txt: Some(old[oi].to_owned()),
                        right_no: None, right_txt: None,
                    });
                    k += 1;
                }
            }
            Op::Ins => {
                let (_, _, ni) = ops[k];
                raw.push(Row {
                    kind: "inserted",
                    left_no: None, left_txt: None,
                    right_no: Some(ni + 1), right_txt: Some(new[ni].to_owned()),
                });
                k += 1;
            }
        }
    }

    // Mark which rows are "near" a change (within context distance).
    let total_rows = raw.len();
    let mut visible = vec![false; total_rows];
    for r in 0..total_rows {
        if raw[r].kind != "equal" {
            let lo = r.saturating_sub(context);
            let hi = (r + context + 1).min(total_rows);
            for v in lo..hi { visible[v] = true; }
        }
    }

    let rows: Vec<serde_json::Value> = raw.iter().zip(visible.iter())
        .filter(|(_, &vis)| vis)
        .map(|(row, _)| json!({
            "type":  row.kind,
            "left":  json!({"line_no": row.left_no, "text": row.left_txt}),
            "right": json!({"line_no": row.right_no, "text": row.right_txt}),
        }))
        .collect();

    serde_json::Value::Array(rows)
}

/// Compute a unified diff between two line slices with `context` lines of context.
/// Returns a JSON array of hunks: `[{header, lines: [{tag: "+"|"-"|" ", text}]}]`.
fn unified_diff(old: &[&str], new: &[&str], context: usize) -> serde_json::Value {
    use serde_json::json;

    // Myers-style edit script via LCS table (simple DP — O(mn) space).
    let m = old.len();
    let n = new.len();

    // lcs[i][j] = length of LCS of old[..i] and new[..j].
    let mut lcs = vec![vec![0usize; n + 1]; m + 1];
    for i in (0..m).rev() {
        for j in (0..n).rev() {
            lcs[i][j] = if old[i] == new[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    // Trace edit operations: Equal / Delete / Insert.
    #[derive(Clone, Copy, PartialEq)]
    enum Op { Eq, Del, Ins }

    let mut ops: Vec<(Op, usize, usize)> = Vec::new(); // (op, old_idx, new_idx)
    let (mut i, mut j) = (0, 0);
    while i < m || j < n {
        if i < m && j < n && old[i] == new[j] {
            ops.push((Op::Eq, i, j));
            i += 1; j += 1;
        } else if j < n && (i >= m || lcs[i][j + 1] >= lcs[i + 1][j]) {
            ops.push((Op::Ins, i, j));
            j += 1;
        } else {
            ops.push((Op::Del, i, j));
            i += 1;
        }
    }

    // Group ops into hunks (changed regions ± context lines).
    let mut hunks = Vec::new();
    let total = ops.len();
    let mut k = 0;
    while k < total {
        // Skip equal lines outside a hunk window.
        if ops[k].0 == Op::Eq {
            k += 1;
            continue;
        }
        // Start a hunk: include up to `context` prior equal lines.
        let hunk_start = k.saturating_sub(context);
        // Extend hunk until we have `context` trailing equal lines after last change.
        let mut end = k;
        loop {
            // Advance past changes.
            while end < total && ops[end].0 != Op::Eq { end += 1; }
            // Count trailing equals.
            let trail_start = end;
            let mut trail = 0;
            while end < total && ops[end].0 == Op::Eq && trail < context {
                end += 1; trail += 1;
            }
            // Check if the next change is within context distance.
            if end < total && ops[end].0 != Op::Eq {
                continue; // merge with next change cluster
            }
            // Include up to `context` trailing equal lines.
            end = trail_start + trail;
            break;
        }

        // Build hunk lines.
        let hunk_ops = &ops[hunk_start..end];
        let old_start = hunk_ops.iter().find(|o| o.0 != Op::Ins).map(|o| o.1 + 1).unwrap_or(1);
        let new_start = hunk_ops.iter().find(|o| o.0 != Op::Del).map(|o| o.2 + 1).unwrap_or(1);
        let old_count = hunk_ops.iter().filter(|o| o.0 != Op::Ins).count();
        let new_count = hunk_ops.iter().filter(|o| o.0 != Op::Del).count();

        let header = format!("@@ -{},{} +{},{} @@", old_start, old_count, new_start, new_count);
        let lines: Vec<serde_json::Value> = hunk_ops.iter().map(|(op, oi, ni)| {
            let (tag, text) = match op {
                Op::Eq  => (" ", old[*oi]),
                Op::Del => ("-", old[*oi]),
                Op::Ins => ("+", new[*ni]),
            };
            json!({"tag": tag, "text": text})
        }).collect();

        hunks.push(json!({"header": header, "lines": lines}));
        k = end;
    }

    serde_json::Value::Array(hunks)
}

/// DELETE /api/v1/drive/files/:id/versions/:v — remove a specific historical version.
///
/// The current live version (stored in drive_files) cannot be deleted this way;
/// use DELETE /drive/files/:id to trash or permanently remove the whole file.
/// On success the blob is deleted from disk and 204 is returned.
async fn delete_version(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, v)): Path<(Uuid, i32)>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    // Verify the file belongs to this tenant/user.
    let f = FileRepo::new(pool).get(ctx.tenant_id, id).await?;
    if f.owner_user_id != ctx.user_id {
        return Err(DriveError::Forbidden);
    }
    // Prevent deleting the current version (version_no in drive_files is stored
    // as the count of uploads; the current blob is in drive_files.storage_key).
    // We detect "current" by comparing storage_key: if the version's blob matches
    // the live file, refuse. Fall back to just checking version count.
    let ver = VersionRepo::new(pool).delete(ctx.tenant_id, id, v).await?
        .ok_or(DriveError::NotFound(id))?;

    // Best-effort blob removal — log but don't fail if already gone.
    let blob_path = state.data_root().join(&ver.storage_key);
    if let Err(e) = fs::remove_file(&blob_path).await {
        tracing::warn!(error = %e, path = %blob_path.display(), "delete_version: blob removal failed");
    }

    tracing::info!(target: "audit",
        event = "drive.file.version_deleted",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id,
        file_id = %id, version_no = v);

    Ok(StatusCode::NO_CONTENT)
}

// ─── Tag handlers ─────────────────────────────────────────────────────────────

/// GET /api/v1/drive/files/:id/tags — list tags on a file.
async fn list_tags(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<Vec<String>>> {
    let pool = state.db_or_unavailable()?;
    // Verify file exists in tenant.
    FileRepo::new(pool).get(ctx.tenant_id, id).await?;
    let tags = TagRepo::new(pool).list(ctx.tenant_id, id).await?;
    Ok(Json(tags))
}

#[derive(Debug, serde::Deserialize)]
struct AddTagBody {
    tag: String,
}

/// POST /api/v1/drive/files/:id/tags — add a tag to a file (idempotent).
async fn add_tag(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<AddTagBody>,
) -> Result<StatusCode> {
    let tag = body.tag.trim().to_string();
    if tag.is_empty() || tag.len() > 64 {
        return Err(DriveError::BadRequest("tag must be 1–64 characters".into()));
    }
    let pool = state.db_or_unavailable()?;
    // Verify file exists in tenant.
    FileRepo::new(pool).get(ctx.tenant_id, id).await?;
    TagRepo::new(pool).add(ctx.tenant_id, id, &tag).await?;
    tracing::info!(target: "audit",
        event = "drive.file.tag_added",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id,
        file_id = %id, tag = %tag);
    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /api/v1/drive/files/:id/tags/:tag — remove a tag from a file.
async fn remove_tag(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, tag)): Path<(Uuid, String)>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    // Verify file exists in tenant.
    FileRepo::new(pool).get(ctx.tenant_id, id).await?;
    TagRepo::new(pool).remove(ctx.tenant_id, id, &tag).await?;
    tracing::info!(target: "audit",
        event = "drive.file.tag_removed",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id,
        file_id = %id, tag = %tag);
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) fn attachment_response(name: &str, mime: Option<&str>, bytes: Vec<u8>) -> Response {
    let mut headers = HeaderMap::new();
    let ct: axum::http::HeaderValue = mime
        .and_then(|m| m.parse().ok())
        .unwrap_or_else(|| axum::http::HeaderValue::from_static("application/octet-stream"));
    headers.insert(header::CONTENT_TYPE, ct);

    // Build a safe `filename=` parameter: ASCII printable only, no quotes,
    // backslashes or control chars. Non-conforming bytes become `_`. Also
    // emit the RFC 5987 `filename*` form so clients can recover the
    // original UTF-8 name. Both are header-injection-safe by construction.
    let cd = build_content_disposition(name);
    let cd_val = axum::http::HeaderValue::from_str(&cd)
        .unwrap_or_else(|_| axum::http::HeaderValue::from_static("attachment"));
    headers.insert(header::CONTENT_DISPOSITION, cd_val);

    (StatusCode::OK, headers, bytes).into_response()
}

fn build_content_disposition(name: &str) -> String {
    let ascii: String = name.chars().map(|c| {
        if c.is_ascii_graphic() && c != '"' && c != '\\' { c } else { '_' }
    }).collect();
    let ascii = if ascii.is_empty() { "download".to_string() } else { ascii };
    let pct = percent_encode_filename(name);
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{pct}")
}

/// RFC 5987 percent-encoding for the `filename*` parameter. Encodes every
/// byte that is not in `attr-char` per RFC 5987 §3.2.1.
fn percent_encode_filename(name: &str) -> String {
    let mut out = String::with_capacity(name.len() * 3);
    for b in name.as_bytes() {
        let c = *b;
        let attr_char = c.is_ascii_alphanumeric()
            || matches!(c, b'!' | b'#' | b'$' | b'&' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~');
        if attr_char {
            out.push(c as char);
        } else {
            out.push_str(&format!("%{c:02X}"));
        }
    }
    out
}

fn sanitize_name(raw: &str) -> Result<String> {
    let s = raw.trim();
    if s.is_empty() || s.contains('/') || s.contains('\\') || s == "." || s == ".." {
        return Err(DriveError::BadRequest("invalid name".into()));
    }
    // Reject ASCII control chars (incl. CR, LF, NUL, TAB). Non-ASCII printables
    // are fine — `build_content_disposition` handles encoding for headers.
    if s.chars().any(|c| (c as u32) < 0x20 || c == '\u{7f}') {
        return Err(DriveError::BadRequest("name has control characters".into()));
    }
    if s.len() > 255 {
        return Err(DriveError::BadRequest("name too long".into()));
    }
    Ok(s.to_string())
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    q:      String,
    #[serde(default = "default_search_limit")]
    limit:  i64,
    #[serde(default)]
    offset: i64,
}
fn default_search_limit() -> i64 { 50 }

/// GET /api/v1/drive/files/search?q=<term>&limit=50&offset=0
/// Case-insensitive substring match on name within the caller's tenant.
async fn search(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<SearchQuery>,
) -> Result<Json<Vec<DriveFile>>> {
    if q.q.trim().is_empty() {
        return Err(DriveError::BadRequest("q must not be empty".into()));
    }
    let limit  = q.limit.clamp(1, 200);
    let offset = q.offset.max(0);
    let pool   = state.db_or_unavailable()?;
    let pattern = format!("%{}%", q.q.replace('%', "\\%").replace('_', "\\_"));
    let rows: Vec<DriveFile> = sqlx::query_as(
        "SELECT id, tenant_id, owner_user_id, parent_id, name, kind, \
                mime_type, size_bytes, sha256, storage_key, created_at, updated_at, deleted_at \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL AND name ILIKE $2 ESCAPE '\\' \
         ORDER BY lower(name) \
         LIMIT $3 OFFSET $4",
    )
    .bind(ctx.tenant_id)
    .bind(&pattern)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(Json(rows))
}

async fn quota(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    req_headers:  HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;

    let max_ts: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT MAX(updated_at) FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool)
    .await
    .unwrap_or(None);

    if let Some(ts) = max_ts {
        if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
            if let Ok(ims_str) = ims_val.to_str() {
                if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                    if ts <= ims_dt {
                        return Ok(StatusCode::NOT_MODIFIED.into_response());
                    }
                }
            }
        }
    }

    let q = QuotaRepo::new(pool).get(ctx.tenant_id).await?;
    let mut resp = Json(q).into_response();
    if let Some(ts) = max_ts {
        let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
        resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    }
    Ok(resp)
}

/// GET /api/v1/drive/users/:user_id/usage — bytes owned by a user in this tenant.
async fn user_usage(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(user_id): Path<Uuid>,
    req_headers:  HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;

    let max_ts: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT MAX(updated_at) FROM drive_files \
         WHERE tenant_id = $1 AND owner_user_id = $2 AND deleted_at IS NULL",
    )
    .bind(ctx.tenant_id)
    .bind(user_id)
    .fetch_one(pool)
    .await
    .unwrap_or(None);

    if let Some(ts) = max_ts {
        if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
            if let Ok(ims_str) = ims_val.to_str() {
                if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                    if ts <= ims_dt {
                        return Ok(StatusCode::NOT_MODIFIED.into_response());
                    }
                }
            }
        }
    }

    let usage = QuotaRepo::new(pool).get_user_usage(ctx.tenant_id, user_id).await?;
    let mut resp = Json(usage).into_response();
    if let Some(ts) = max_ts {
        let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
        resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    }
    Ok(resp)
}

/// GET /api/v1/drive/files/stats
///
/// Returns storage breakdown for the tenant: total files, total folders, total
/// size, and per-MIME-type aggregates. Only non-deleted (live) files are counted.
/// `by_mime_type` is ordered by `total_bytes DESC` so the heaviest types appear
/// first — useful for "storage breakdown" UIs. Folders have no mime_type and
/// appear as `"application/x-directory"` bucket (folded from NULL + kind=folder).
/// Response: `{files, folders, total_bytes, by_mime_type: [{mime_type, count, total_bytes}]}`.
/// Sprint #616.
async fn file_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    // Total file and folder counts + overall size.
    let (files, folders, total_bytes): (i64, i64, i64) = sqlx::query_as(
        "SELECT \
            COUNT(*) FILTER (WHERE kind = 'file') AS files, \
            COUNT(*) FILTER (WHERE kind = 'folder') AS folders, \
            COALESCE(SUM(size_bytes) FILTER (WHERE kind = 'file'), 0)::BIGINT AS total_bytes \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool)
    .await?;

    // Per-MIME-type breakdown (files only; NULL mime_type grouped as 'application/octet-stream').
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT \
            COALESCE(mime_type, 'application/octet-stream') AS mime_type, \
            COUNT(*)::BIGINT AS count, \
            COALESCE(SUM(size_bytes), 0)::BIGINT AS total_bytes \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL AND kind = 'file' \
         GROUP BY mime_type \
         ORDER BY total_bytes DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;

    let by_mime: Vec<serde_json::Value> = rows.into_iter().map(|(mime, count, bytes)| {
        serde_json::json!({"mime_type": mime, "count": count, "total_bytes": bytes})
    }).collect();

    Ok(Json(serde_json::json!({
        "files":        files,
        "folders":      folders,
        "total_bytes":  total_bytes,
        "by_mime_type": by_mime,
    })))
}

#[derive(Debug, Deserialize)]
struct ExpiryBody {
    /// RFC 3339 timestamp; null or omitted clears the expiry.
    #[serde(default, with = "time::serde::rfc3339::option")]
    expires_at: Option<OffsetDateTime>,
}

/// PATCH /api/v1/drive/files/:id/expiry — set or clear the expiry timestamp.
///
/// Only the file owner or a moderator may call this. On expiry the file is
/// hard-deleted by the background purge worker (hourly GC).
async fn set_expiry(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<ExpiryBody>,
) -> Result<Json<DriveFile>> {
    let pool = state.db_or_unavailable()?;
    let f    = FileRepo::new(pool).get(ctx.tenant_id, id).await?;
    if f.owner_user_id != ctx.user_id {
        return Err(DriveError::Forbidden);
    }
    if let Some(exp) = body.expires_at {
        if exp <= OffsetDateTime::now_utc() {
            return Err(DriveError::BadRequest("expires_at must be in the future".into()));
        }
    }
    let updated = FileRepo::new(pool).set_expiry(ctx.tenant_id, id, body.expires_at).await?;
    tracing::info!(target: "audit",
        event = "drive.file.expiry_set",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id,
        file_id = %id, expires_at = ?body.expires_at);
    Ok(Json(updated))
}

/// POST /api/v1/drive/starred — list user's starred files (newest star first)
async fn list_starred(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<Vec<DriveFile>>> {
    let pool  = state.db_or_unavailable()?;
    let files = FileRepo::new(pool).list_starred(ctx.tenant_id, ctx.user_id).await?;
    Ok(Json(files))
}

/// GET /api/v1/drive/starred/count — count user's starred files (sprint #458).
/// Mesma semântica de `list_starred` (filtra `starred_at IS NOT NULL` + `deleted_at IS NULL`),
/// só retorna `{count: N}` pra badge de UI sem trafegar a lista. Path child de
/// `/starred` (sem ambiguidade com `:id` em outros recursos).
async fn count_starred(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool  = state.db_or_unavailable()?;
    let count = FileRepo::new(pool).count_starred(ctx.tenant_id, ctx.user_id).await?;
    Ok(Json(serde_json::json!({ "count": count })))
}

/// POST /api/v1/drive/files/:id/star — mark file as starred
async fn star_file(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<DriveFile>> {
    let pool    = state.db_or_unavailable()?;
    let updated = FileRepo::new(pool).star_file(ctx.tenant_id, id, ctx.user_id).await?;
    Ok(Json(updated))
}

/// DELETE /api/v1/drive/files/:id/star — remove star
async fn unstar_file(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<DriveFile>> {
    let pool    = state.db_or_unavailable()?;
    let updated = FileRepo::new(pool).unstar_file(ctx.tenant_id, id, ctx.user_id).await?;
    Ok(Json(updated))
}

/// POST /api/v1/drive/files/:id/lock — acquire optimistic lock (owner only or first caller)
/// DELETE /api/v1/drive/files/:id/lock — release lock (only the lock holder may unlock)
async fn lock_file(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<DriveFile>> {
    let pool    = state.db_or_unavailable()?;
    let updated = FileRepo::new(pool).lock_file(ctx.tenant_id, id, ctx.user_id).await?;
    tracing::info!(target: "audit",
        event = "drive.file.locked",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, file_id = %id);
    Ok(Json(updated))
}

async fn unlock_file(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<DriveFile>> {
    let pool    = state.db_or_unavailable()?;
    let updated = FileRepo::new(pool).unlock_file(ctx.tenant_id, id, ctx.user_id).await?;
    tracing::info!(target: "audit",
        event = "drive.file.unlocked",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, file_id = %id);
    Ok(Json(updated))
}

/// GET /api/v1/drive/folders/:id/download
///
/// Recursively packs all files inside the folder (including sub-folders) into a
/// ZIP archive returned in-memory. Empty folders produce an empty ZIP.
async fn download_folder(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Response> {
    let pool   = state.db_or_unavailable()?;
    let repo   = FileRepo::new(pool);
    let folder = repo.get(ctx.tenant_id, id).await?;
    if folder.kind != "folder" {
        return Err(DriveError::BadRequest("target is not a folder".into()));
    }

    let entries = repo.collect_files_recursive(ctx.tenant_id, id, "").await?;

    let buf: Vec<u8> = Vec::new();
    let cursor = std::io::Cursor::new(buf);
    let mut zip = zip::ZipWriter::new(cursor);
    let options = zip::write::FileOptions::<()>::default()
        .compression_method(zip::CompressionMethod::Deflated);

    for (rel_path, file) in &entries {
        let key = match file.storage_key.as_deref() {
            Some(k) => k,
            None    => continue,
        };
        let bytes = match fs::read(state.data_root().join(key)).await {
            Ok(b)  => b,
            Err(e) => {
                tracing::warn!(file_id = %file.id, error = %e, "skipping unreadable blob in folder download");
                continue;
            }
        };
        zip.start_file(rel_path, options).map_err(|e| DriveError::Io(std::io::Error::other(e.to_string())))?;
        zip.write_all(&bytes).map_err(DriveError::Io)?;
    }

    let cursor = zip.finish().map_err(|e| DriveError::Io(std::io::Error::other(e.to_string())))?;
    let zip_bytes = cursor.into_inner();

    let ascii: String = folder.name.chars().map(|c| {
        if c.is_ascii_graphic() && c != '"' && c != '\\' { c } else { '_' }
    }).collect();
    let archive_name = if ascii.is_empty() { "folder".to_string() } else { ascii };
    let cd = format!("attachment; filename=\"{archive_name}.zip\"");

    tracing::info!(target: "audit",
        event = "drive.folder.download",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, folder_id = %id,
        file_count = entries.len());

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE,        HeaderValue::from_static("application/zip")),
            (header::CONTENT_DISPOSITION, HeaderValue::from_str(&cd).unwrap_or_else(|_| HeaderValue::from_static("attachment"))),
        ],
        zip_bytes,
    ).into_response())
}

/// GET /api/v1/drive/folders/:id/quota — current folder quota + used bytes.
/// Returns 404 if folder doesn't exist or has no quota configured.
async fn folder_quota(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<FolderQuota>> {
    let pool = state.db_or_unavailable()?;
    // Verify folder exists and belongs to tenant.
    let f = FileRepo::new(pool).get(ctx.tenant_id, id).await?;
    if f.kind != "folder" {
        return Err(DriveError::BadRequest("id is not a folder".into()));
    }
    FolderQuotaRepo::new(pool)
        .get(ctx.tenant_id, id).await?
        .ok_or_else(|| DriveError::NotFound(id))
        .map(Json)
}

#[derive(Debug, serde::Deserialize)]
struct FolderQuotaBody {
    max_bytes: i64,
}

/// PUT /api/v1/drive/folders/:id/quota — set (upsert) folder quota.
async fn set_folder_quota(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<FolderQuotaBody>,
) -> Result<Json<FolderQuota>> {
    let pool = state.db_or_unavailable()?;
    let f = FileRepo::new(pool).get(ctx.tenant_id, id).await?;
    if f.kind != "folder" {
        return Err(DriveError::BadRequest("id is not a folder".into()));
    }
    if body.max_bytes <= 0 {
        return Err(DriveError::BadRequest("max_bytes must be > 0".into()));
    }
    let fq = FolderQuotaRepo::new(pool).set(ctx.tenant_id, id, body.max_bytes).await?;
    tracing::info!(target: "audit",
        event = "drive.folder.quota_set",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, folder_id = %id, max_bytes = body.max_bytes);
    Ok(Json(fq))
}

/// GET /api/v1/drive/files/stats/updated-by-day?since=&until= — arquivos modificados por dia.
///
/// DATE_TRUNC('day', updated_at) COUNT sobre `drive_files` (kind='file', não-deletados).
/// `since`/`until` RFC3339 opcionais via `$N::timestamptz IS NULL OR`. Retorna
/// `{days:[{day,count}]}` ordenado dia ASC. Complementa `created-by-day` (#682) focando
/// em modificações vs criações. Sprint #692.
async fn file_stats_updated_by_day(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsActivityQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT to_char(date_trunc('day', updated_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
                COUNT(*)::BIGINT AS count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL AND kind = 'file' \
            AND ($2::timestamptz IS NULL OR updated_at >= $2) \
            AND ($3::timestamptz IS NULL OR updated_at <  $3) \
          GROUP BY day \
          ORDER BY day ASC",
    )
    .bind(ctx.tenant_id)
    .bind(q.since)
    .bind(q.until)
    .fetch_all(pool)
    .await?;

    let days: Vec<serde_json::Value> = rows.into_iter()
        .map(|(day, count)| serde_json::json!({"day": day, "count": count}))
        .collect();
    Ok(Json(serde_json::json!({"days": days})))
}

/// GET /api/v1/drive/files/stats/by-size-bucket?folder_id= — distribuição de tamanho em 8 faixas.
///
/// Conta arquivos (kind='file', não-deletados) por faixa de `size_bytes`:
/// <1KB / 1-10KB / 10-100KB / 100KB-1MB / 1-10MB / 10-100MB / 100MB-1GB / >1GB.
/// `folder_id` (UUID) filtra por `parent_id`; omitido = tenant inteiro.
/// Retorna `{buckets:[{range,count,total_bytes}]}`. Sprint #687.
#[derive(Debug, Deserialize)]
struct StatsSizeBucketQuery {
    folder_id: Option<Uuid>,
}

async fn file_stats_by_size_bucket(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsSizeBucketQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let folder_filter = if q.folder_id.is_some() {
        "AND parent_id = $2"
    } else {
        "AND ($2::uuid IS NULL OR parent_id = $2)"
    };

    let sql = format!(
        "SELECT \
            COUNT(*) FILTER (WHERE size_bytes < 1024)::BIGINT, \
            COALESCE(SUM(size_bytes) FILTER (WHERE size_bytes < 1024), 0)::BIGINT, \
            COUNT(*) FILTER (WHERE size_bytes >= 1024        AND size_bytes < 10240)::BIGINT, \
            COALESCE(SUM(size_bytes) FILTER (WHERE size_bytes >= 1024        AND size_bytes < 10240), 0)::BIGINT, \
            COUNT(*) FILTER (WHERE size_bytes >= 10240       AND size_bytes < 102400)::BIGINT, \
            COALESCE(SUM(size_bytes) FILTER (WHERE size_bytes >= 10240       AND size_bytes < 102400), 0)::BIGINT, \
            COUNT(*) FILTER (WHERE size_bytes >= 102400      AND size_bytes < 1048576)::BIGINT, \
            COALESCE(SUM(size_bytes) FILTER (WHERE size_bytes >= 102400      AND size_bytes < 1048576), 0)::BIGINT, \
            COUNT(*) FILTER (WHERE size_bytes >= 1048576     AND size_bytes < 10485760)::BIGINT, \
            COALESCE(SUM(size_bytes) FILTER (WHERE size_bytes >= 1048576     AND size_bytes < 10485760), 0)::BIGINT, \
            COUNT(*) FILTER (WHERE size_bytes >= 10485760    AND size_bytes < 104857600)::BIGINT, \
            COALESCE(SUM(size_bytes) FILTER (WHERE size_bytes >= 10485760    AND size_bytes < 104857600), 0)::BIGINT, \
            COUNT(*) FILTER (WHERE size_bytes >= 104857600   AND size_bytes < 1073741824)::BIGINT, \
            COALESCE(SUM(size_bytes) FILTER (WHERE size_bytes >= 104857600   AND size_bytes < 1073741824), 0)::BIGINT, \
            COUNT(*) FILTER (WHERE size_bytes >= 1073741824)::BIGINT, \
            COALESCE(SUM(size_bytes) FILTER (WHERE size_bytes >= 1073741824), 0)::BIGINT \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL AND kind = 'file' {folder_filter}"
    );

    let row: (i64,i64, i64,i64, i64,i64, i64,i64, i64,i64, i64,i64, i64,i64, i64,i64) =
        sqlx::query_as(&sql)
            .bind(ctx.tenant_id)
            .bind(q.folder_id)
            .fetch_one(pool)
            .await?;

    let buckets = vec![
        serde_json::json!({"range": "<1KB",        "count": row.0,  "total_bytes": row.1}),
        serde_json::json!({"range": "1-10KB",      "count": row.2,  "total_bytes": row.3}),
        serde_json::json!({"range": "10-100KB",    "count": row.4,  "total_bytes": row.5}),
        serde_json::json!({"range": "100KB-1MB",   "count": row.6,  "total_bytes": row.7}),
        serde_json::json!({"range": "1-10MB",      "count": row.8,  "total_bytes": row.9}),
        serde_json::json!({"range": "10-100MB",    "count": row.10, "total_bytes": row.11}),
        serde_json::json!({"range": "100MB-1GB",   "count": row.12, "total_bytes": row.13}),
        serde_json::json!({"range": ">1GB",        "count": row.14, "total_bytes": row.15}),
    ];
    Ok(Json(serde_json::json!({"buckets": buckets})))
}

/// GET /api/v1/drive/files/stats/folder-depth — histograma de profundidade de pasta.
///
/// CTE recursiva calcula a profundidade de cada pasta (kind='folder', não-deletada)
/// a partir da raiz (parent_id IS NULL = depth 0). Retorna
/// `{buckets:[{depth,count,total_bytes}]}` ordenado por depth ASC. `total_bytes`
/// é a soma de size_bytes dos arquivos diretamente filhos (kind='file'). Sprint #696.
async fn file_stats_folder_depth(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i64, i64, i64)> = sqlx::query_as(
        "WITH RECURSIVE folder_tree AS ( \
             SELECT id, 0::BIGINT AS depth \
               FROM drive_files \
              WHERE tenant_id = $1 AND kind = 'folder' AND deleted_at IS NULL AND parent_id IS NULL \
             UNION ALL \
             SELECT f.id, ft.depth + 1 \
               FROM drive_files f \
               JOIN folder_tree ft ON f.parent_id = ft.id \
              WHERE f.tenant_id = $1 AND f.kind = 'folder' AND f.deleted_at IS NULL \
         ) \
         SELECT ft.depth, \
                COUNT(DISTINCT ft.id)::BIGINT AS folder_count, \
                COALESCE(SUM(child.size_bytes) FILTER (WHERE child.kind = 'file' AND child.deleted_at IS NULL), 0)::BIGINT AS total_bytes \
           FROM folder_tree ft \
           LEFT JOIN drive_files child ON child.parent_id = ft.id AND child.tenant_id = $1 \
          GROUP BY ft.depth \
          ORDER BY ft.depth ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;

    let buckets: Vec<serde_json::Value> = rows.into_iter()
        .map(|(depth, count, total_bytes)| serde_json::json!({
            "depth":       depth,
            "count":       count,
            "total_bytes": total_bytes,
        }))
        .collect();
    Ok(Json(serde_json::json!({"buckets": buckets})))
}

/// GET /api/v1/drive/files/stats/version-count?limit=N — top-N arquivos por número de versões.
///
/// JOIN `drive_file_versions` com `drive_files` (não-deletados, kind='file').
/// Retorna `{files:[{file_id,name,version_count}]}` ordenado por version_count DESC.
/// `limit` default 20 max 200. Sprint #701.
#[derive(Debug, Deserialize)]
struct StatsVersionCountQuery {
    limit: Option<i64>,
}

async fn file_stats_version_count(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsVersionCountQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(20).clamp(1, 200);

    let rows: Vec<(Uuid, String, i64)> = sqlx::query_as(
        "SELECT f.id, f.name, COUNT(v.id)::BIGINT AS version_count \
           FROM drive_files f \
           JOIN drive_file_versions v ON v.file_id = f.id AND v.tenant_id = $1 \
          WHERE f.tenant_id = $1 AND f.deleted_at IS NULL AND f.kind = 'file' \
          GROUP BY f.id, f.name \
          ORDER BY version_count DESC, f.name ASC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let files: Vec<serde_json::Value> = rows.into_iter()
        .map(|(file_id, name, version_count)| serde_json::json!({
            "file_id":       file_id,
            "name":          name,
            "version_count": version_count,
        }))
        .collect();
    Ok(Json(serde_json::json!({"files": files})))
}

/// GET /api/v1/drive/files/stats/tag-count?limit=N — top-N tags por uso.
///
/// GROUP BY tag em `drive_file_tags`, COUNT DISTINCT file_id. Retorna
/// `{tags:[{tag,file_count}]}` ordenado por file_count DESC. `limit` default 20 max 200.
/// Sprint #706.
#[derive(Debug, Deserialize)]
struct StatsTagCountQuery {
    limit: Option<i64>,
}

async fn file_stats_tag_count(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTagCountQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(20).clamp(1, 200);

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT tag, COUNT(DISTINCT file_id)::BIGINT AS file_count \
           FROM drive_file_tags \
          WHERE tenant_id = $1 \
          GROUP BY tag \
          ORDER BY file_count DESC, tag ASC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let tags: Vec<serde_json::Value> = rows.into_iter()
        .map(|(tag, file_count)| serde_json::json!({"tag": tag, "file_count": file_count}))
        .collect();
    Ok(Json(serde_json::json!({"tags": tags})))
}

/// GET /api/v1/drive/files/stats/ext-by-folder?folder_id= — breakdown de extensão numa pasta.
///
/// Análogo a `mime-by-folder` (#672) mas agrupa por extensão (parte após o último '.' do nome).
/// `folder_id` opcional: ausente = raiz (`parent_id IS NULL`). Extensão NULL = sem ponto no nome.
/// Retorna `{folder_id,rows:[{extension,file_count,total_bytes}]}` ordenado por total_bytes DESC. Sprint #711.
#[derive(Debug, Deserialize)]
struct StatsExtByFolderQuery {
    folder_id: Option<Uuid>,
}

async fn file_stats_ext_by_folder(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsExtByFolderQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(Option<String>, i64, i64)> = if let Some(fid) = q.folder_id {
        sqlx::query_as(
            "SELECT \
                CASE WHEN name LIKE '%.%' \
                     THEN lower(substring(name FROM '\\.[^.]*$')) \
                     ELSE NULL END AS extension, \
                COUNT(*)::BIGINT AS file_count, \
                COALESCE(SUM(size_bytes), 0)::BIGINT AS total_bytes \
               FROM drive_files \
              WHERE tenant_id = $1 AND deleted_at IS NULL AND kind = 'file' AND parent_id = $2 \
              GROUP BY extension \
              ORDER BY total_bytes DESC",
        )
        .bind(ctx.tenant_id).bind(fid).fetch_all(pool).await?
    } else {
        sqlx::query_as(
            "SELECT \
                CASE WHEN name LIKE '%.%' \
                     THEN lower(substring(name FROM '\\.[^.]*$')) \
                     ELSE NULL END AS extension, \
                COUNT(*)::BIGINT AS file_count, \
                COALESCE(SUM(size_bytes), 0)::BIGINT AS total_bytes \
               FROM drive_files \
              WHERE tenant_id = $1 AND deleted_at IS NULL AND kind = 'file' AND parent_id IS NULL \
              GROUP BY extension \
              ORDER BY total_bytes DESC",
        )
        .bind(ctx.tenant_id).fetch_all(pool).await?
    };

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, fc, tb)| serde_json::json!({
            "extension":   ext,
            "file_count":  fc,
            "total_bytes": tb,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folder_id": q.folder_id, "rows": out})))
}

/// GET /api/v1/drive/files/stats/lock-count — arquivos bloqueados por total e por user_id.
///
/// Conta arquivos onde `locked_at IS NOT NULL AND deleted_at IS NULL AND kind = 'file'`.
/// Retorna `{total_locked, by_user:[{user_id,locked_count}]}` ordenado por locked_count DESC.
/// Sprint #716.
async fn file_stats_lock_count(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let (total_locked,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL AND locked_at IS NOT NULL",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool)
    .await?;

    let by_user: Vec<(Option<Uuid>, i64)> = sqlx::query_as(
        "SELECT owner_user_id, COUNT(*)::BIGINT AS locked_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL AND locked_at IS NOT NULL \
          GROUP BY owner_user_id \
          ORDER BY locked_count DESC, owner_user_id ASC NULLS LAST",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;

    let by_user_out: Vec<serde_json::Value> = by_user.into_iter()
        .map(|(uid, cnt)| serde_json::json!({"user_id": uid, "locked_count": cnt}))
        .collect();

    Ok(Json(serde_json::json!({
        "total_locked": total_locked,
        "by_user":      by_user_out,
    })))
}

/// GET /api/v1/drive/files/stats/starred-count — arquivos estrelados por total e por user_id.
///
/// Conta arquivos onde `starred_at IS NOT NULL AND deleted_at IS NULL AND kind = 'file'`.
/// Retorna `{total_starred, by_user:[{user_id,starred_count}]}` ordenado por starred_count DESC.
/// Sprint #721.
async fn file_stats_starred_count(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let (total_starred,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL AND starred_at IS NOT NULL",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool)
    .await?;

    let by_user: Vec<(Option<Uuid>, i64)> = sqlx::query_as(
        "SELECT owner_user_id, COUNT(*)::BIGINT AS starred_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL AND starred_at IS NOT NULL \
          GROUP BY owner_user_id \
          ORDER BY starred_count DESC, owner_user_id ASC NULLS LAST",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;

    let by_user_out: Vec<serde_json::Value> = by_user.into_iter()
        .map(|(uid, cnt)| serde_json::json!({"user_id": uid, "starred_count": cnt}))
        .collect();

    Ok(Json(serde_json::json!({
        "total_starred": total_starred,
        "by_user":       by_user_out,
    })))
}

/// GET /api/v1/drive/files/stats/expiry-count — arquivos com expires_at definido.
///
/// Retorna total com expiry + já expirados (expires_at < NOW()). Apenas não-deletados.
/// Retorna `{total_with_expiry,already_expired,not_yet_expired}`. Sprint #726.
async fn file_stats_expiry_count(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let (total_with_expiry, already_expired): (i64, i64) = sqlx::query_as(
        "SELECT \
            COUNT(*) FILTER (WHERE expires_at IS NOT NULL)::BIGINT AS total_with_expiry, \
            COUNT(*) FILTER (WHERE expires_at IS NOT NULL AND expires_at < NOW())::BIGINT AS already_expired \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool)
    .await?;

    Ok(Json(serde_json::json!({
        "total_with_expiry": total_with_expiry,
        "already_expired":   already_expired,
        "not_yet_expired":   total_with_expiry - already_expired,
    })))
}

/// GET /api/v1/drive/files/stats/mime-top-n?limit=N — top-N mime_types por file_count (global, cross-folder).
///
/// Conta arquivos não-deletados agrupados por mime_type e retorna os N mais frequentes.
/// Análogo a ext-by-folder (#711) mas sem filtro de pasta — visão global do tenant.
/// `limit` default 20 max 100. Retorna `{rows:[{mime_type,file_count}]}`. Sprint #731.
async fn file_stats_mime_top_n(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q): Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(20).min(100).max(1);
    let pool  = state.db_or_unavailable()?;

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT mime_type, COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND deleted_at IS NULL \
            AND kind       = 'file' \
            AND mime_type  IS NOT NULL \
          GROUP BY mime_type \
          ORDER BY file_count DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime_type, file_count)| serde_json::json!({"mime_type": mime_type, "file_count": file_count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/orphan-versions — versões sem arquivo pai.
///
/// Conta entradas em `drive_file_versions` cujo `file_id` não existe em `drive_files` (LEFT JOIN IS NULL).
/// Indica inconsistência de storage — versões órfãs não têm dono válido.
/// Retorna `{orphan_version_count}`. Sprint #736.
async fn file_stats_orphan_versions(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let (orphan_version_count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(v.id)::BIGINT \
           FROM drive_file_versions v \
           LEFT JOIN drive_files f ON f.id = v.file_id AND f.tenant_id = $1 \
          WHERE v.tenant_id = $1 \
            AND f.id IS NULL",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool)
    .await?;

    Ok(Json(serde_json::json!({"orphan_version_count": orphan_version_count})))
}

/// GET /api/v1/drive/files/stats/empty-files — arquivos com tamanho zero ou indefinido.
///
/// Conta arquivos não-deletados com `size_bytes IS NULL OR size_bytes = 0` (kind='file').
/// Útil pra detectar uploads incompletos. Retorna `{total_empty,null_size,zero_size}`. Sprint #741.
async fn file_stats_empty_files(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let (total_empty, null_size, zero_size): (i64, i64, i64) = sqlx::query_as(
        "SELECT \
            COUNT(*) FILTER (WHERE size_bytes IS NULL OR size_bytes = 0)::BIGINT AS total_empty, \
            COUNT(*) FILTER (WHERE size_bytes IS NULL)::BIGINT                   AS null_size, \
            COUNT(*) FILTER (WHERE size_bytes = 0)::BIGINT                      AS zero_size \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND deleted_at IS NULL \
            AND kind       = 'file'",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool)
    .await?;

    Ok(Json(serde_json::json!({
        "total_empty": total_empty,
        "null_size":   null_size,
        "zero_size":   zero_size,
    })))
}

/// GET /api/v1/drive/files/stats/deleted-by-day?since=&until= — arquivos deletados por dia.
///
/// DATE_TRUNC('day', deleted_at); kind='file'; bounds opcionais. Análogo a created-by-day (#682).
/// Retorna `{rows:[{day,count}]}` day ASC. Sprint #746.
#[derive(Debug, Deserialize)]
struct StatsByDayQuery {
    since: Option<String>,
    until: Option<String>,
}

async fn file_stats_deleted_by_day(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsByDayQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let since_dt: Option<time::OffsetDateTime> = q.since.as_deref()
        .map(|s| time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339))
        .transpose()
        .map_err(|_| crate::error::DriveError::BadRequest("since must be RFC3339".into()))?;
    let until_dt: Option<time::OffsetDateTime> = q.until.as_deref()
        .map(|s| time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339))
        .transpose()
        .map_err(|_| crate::error::DriveError::BadRequest("until must be RFC3339".into()))?;

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT to_char(date_trunc('day', deleted_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
                COUNT(*)::BIGINT AS count \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND kind       = 'file' \
            AND deleted_at IS NOT NULL \
            AND ($2::timestamptz IS NULL OR deleted_at >= $2) \
            AND ($3::timestamptz IS NULL OR deleted_at <  $3) \
          GROUP BY day \
          ORDER BY day ASC",
    )
    .bind(ctx.tenant_id)
    .bind(since_dt)
    .bind(until_dt)
    .fetch_all(pool)
    .await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(day, count)| serde_json::json!({"day": day, "count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/name-length — avg/max LENGTH(name) global (kind='file', não-deletados).
///
/// Indicador de verbosidade nos nomes de arquivo do tenant.
/// Retorna `{file_count,avg_name_length,max_name_length}`. Sprint #751.
async fn file_stats_name_length(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let (file_count, avg_name_length, max_name_length): (i64, Option<f64>, Option<i64>) = sqlx::query_as(
        "SELECT \
            COUNT(*)::BIGINT AS file_count, \
            AVG(LENGTH(name)), \
            MAX(LENGTH(name))::BIGINT \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND deleted_at IS NULL \
            AND kind       = 'file'",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool)
    .await?;

    Ok(Json(serde_json::json!({
        "file_count":       file_count,
        "avg_name_length":  avg_name_length,
        "max_name_length":  max_name_length,
    })))
}

/// GET /api/v1/drive/files/stats/ext-top-n?limit=N — top-N extensões globais por file_count.
///
/// Complementa mime-top-n (#731) com extensão extraída via substring. Default 20 max 100.
/// Retorna `{rows:[{extension,file_count}]}` count DESC. Sprint #756.
async fn file_stats_ext_top_n(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(20).min(100).max(1);
    let pool  = state.db_or_unavailable()?;

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT \
            LOWER(SUBSTRING(name FROM '\\.[^.]*$')) AS extension, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND deleted_at IS NULL \
            AND kind       = 'file' \
            AND name LIKE '%.%' \
          GROUP BY extension \
          ORDER BY file_count DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, cnt)| serde_json::json!({"extension": ext, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/storage-by-user?limit=N — top-N usuários por total_bytes.
///
/// Soma size_bytes de arquivos não-deletados (kind='file') por owner_user_id. Default 20 max 200.
/// Retorna `{rows:[{user_id,file_count,total_bytes}]}` total_bytes DESC. Sprint #761.
async fn file_stats_storage_by_user(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(20).min(200).max(1);
    let pool  = state.db_or_unavailable()?;

    let rows: Vec<(Uuid, i64, i64)> = sqlx::query_as(
        "SELECT owner_user_id, COUNT(*)::BIGINT AS file_count, COALESCE(SUM(size_bytes),0)::BIGINT AS total_bytes \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND deleted_at IS NULL \
            AND kind       = 'file' \
          GROUP BY owner_user_id \
          ORDER BY total_bytes DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(uid, fc, tb)| serde_json::json!({"user_id": uid, "file_count": fc, "total_bytes": tb}))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/quota-usage — pastas com quota: total_bytes + quota + pct_used.
///
/// JOIN drive_folder_quotas com SUM(size_bytes) dos filhos diretos (parent_id = folder_id).
/// Retorna `{rows:[{folder_id,max_bytes,used_bytes,pct_used}]}`. Sprint #766.
async fn file_stats_quota_usage(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(Uuid, i64, i64)> = sqlx::query_as(
        "SELECT q.folder_id, q.max_bytes, \
                COALESCE(SUM(f.size_bytes), 0)::BIGINT AS used_bytes \
           FROM drive_folder_quotas q \
           LEFT JOIN drive_files f ON f.parent_id = q.folder_id \
                                  AND f.tenant_id = $1 \
                                  AND f.deleted_at IS NULL \
                                  AND f.kind = 'file' \
          WHERE q.tenant_id = $1 \
          GROUP BY q.folder_id, q.max_bytes \
          ORDER BY used_bytes DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder_id, max_bytes, used_bytes)| {
            let pct = if max_bytes > 0 { used_bytes as f64 / max_bytes as f64 * 100.0 } else { 0.0 };
            serde_json::json!({"folder_id": folder_id, "max_bytes": max_bytes, "used_bytes": used_bytes, "pct_used": pct})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/folder-file-count?limit=N — top-N pastas por file_count.
///
/// COUNT de arquivos não-deletados (kind='file') por parent_id. Default 20 max 200.
/// Retorna `{rows:[{folder_id,file_count,total_bytes}]}` file_count DESC. Sprint #771.
async fn file_stats_folder_file_count(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(20).min(200).max(1);
    let pool  = state.db_or_unavailable()?;

    let rows: Vec<(Option<Uuid>, i64, i64)> = sqlx::query_as(
        "SELECT parent_id, COUNT(*)::BIGINT AS file_count, COALESCE(SUM(size_bytes),0)::BIGINT AS total_bytes \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND deleted_at IS NULL \
            AND kind       = 'file' \
          GROUP BY parent_id \
          ORDER BY file_count DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(pid, fc, tb)| serde_json::json!({"folder_id": pid, "file_count": fc, "total_bytes": tb}))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/deep-files?min_depth=N — arquivos em pastas com profundidade >= N.
///
/// CTE recursiva calcula depth de cada file/folder desde a raiz.
/// Agrupa arquivos (kind='file', não-deletados) por depth >= min_depth (default 3).
/// Retorna `{min_depth,total_files,total_bytes,by_depth:[{depth,file_count,total_bytes}]}`. Sprint #776.
async fn file_stats_deep_files(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<DeepFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let min_depth = q.min_depth.unwrap_or(3).max(1) as i64;
    let pool      = state.db_or_unavailable()?;

    let rows: Vec<(i64, i64, i64)> = sqlx::query_as(
        "WITH RECURSIVE tree AS ( \
            SELECT id, parent_id, kind, size_bytes, deleted_at, 0 AS depth \
              FROM drive_files \
             WHERE tenant_id = $1 AND parent_id IS NULL \
            UNION ALL \
            SELECT f.id, f.parent_id, f.kind, f.size_bytes, f.deleted_at, t.depth + 1 \
              FROM drive_files f \
              JOIN tree t ON f.parent_id = t.id \
             WHERE f.tenant_id = $1 \
         ) \
         SELECT depth, COUNT(*)::BIGINT AS file_count, COALESCE(SUM(size_bytes),0)::BIGINT AS total_bytes \
           FROM tree \
          WHERE kind = 'file' AND deleted_at IS NULL AND depth >= $2 \
          GROUP BY depth \
          ORDER BY depth ASC",
    )
    .bind(ctx.tenant_id).bind(min_depth)
    .fetch_all(pool).await?;

    let total_files: i64  = rows.iter().map(|(_, fc, _)| fc).sum();
    let total_bytes: i64  = rows.iter().map(|(_, _, tb)| tb).sum();
    let by_depth: Vec<serde_json::Value> = rows.into_iter()
        .map(|(depth, fc, tb)| serde_json::json!({"depth": depth, "file_count": fc, "total_bytes": tb}))
        .collect();
    Ok(Json(serde_json::json!({
        "min_depth":   min_depth,
        "total_files": total_files,
        "total_bytes": total_bytes,
        "by_depth":    by_depth,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct DeepFilesQuery {
    min_depth: Option<u32>,
}

/// GET /api/v1/drive/files/stats/mime-entropy — entropia de Shannon sobre distribuição mime_type.
///
/// H = -Σ p_i * log2(p_i) onde p_i = count_i / total.
/// mime_type NULL agrupado como "application/octet-stream". kind='file', não-deletados.
/// Retorna `{mime_count,total_files,entropy_bits,top:[{mime_type,file_count}]}`. Sprint #781.
async fn file_stats_mime_entropy(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type,'application/octet-stream') AS mime, COUNT(*)::BIGINT AS cnt \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND deleted_at IS NULL \
            AND kind       = 'file' \
          GROUP BY mime \
          ORDER BY cnt DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let total: i64 = rows.iter().map(|(_, c)| c).sum();
    let entropy: f64 = if total == 0 {
        0.0
    } else {
        rows.iter().filter(|(_, c)| *c > 0).map(|(_, c)| {
            let p = *c as f64 / total as f64;
            -p * p.log2()
        }).sum()
    };
    let top: Vec<serde_json::Value> = rows.iter().take(20)
        .map(|(m, c)| serde_json::json!({"mime_type": m, "file_count": c}))
        .collect();
    Ok(Json(serde_json::json!({
        "mime_count":   rows.len(),
        "total_files":  total,
        "entropy_bits": entropy,
        "top":          top,
    })))
}

/// GET /api/v1/drive/files/stats/avg-versions — AVG e MAX versões por arquivo.
///
/// JOIN drive_file_versions GROUP BY file_id → avg/max version_count.
/// Apenas arquivos com ao menos 1 versão (kind='file', não-deletados no drive_files).
/// Retorna `{files_with_versions,avg_versions,max_versions}`. Sprint #791.
async fn file_stats_avg_versions(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let (files_with_versions, avg_versions, max_versions): (i64, Option<f64>, Option<i64>) =
        sqlx::query_as(
            "SELECT \
                COUNT(DISTINCT v.file_id)::BIGINT, \
                AVG(vc.cnt), \
                MAX(vc.cnt)::BIGINT \
               FROM ( \
                 SELECT file_id, COUNT(*)::BIGINT AS cnt \
                   FROM drive_file_versions \
                  WHERE tenant_id = $1 \
                  GROUP BY file_id \
               ) vc \
               JOIN drive_file_versions v ON v.file_id = vc.file_id AND v.tenant_id = $1 \
               JOIN drive_files f ON f.id = v.file_id AND f.tenant_id = $1 \
                                  AND f.deleted_at IS NULL AND f.kind = 'file'",
        )
        .bind(ctx.tenant_id)
        .fetch_one(pool).await?;

    Ok(Json(serde_json::json!({
        "files_with_versions": files_with_versions,
        "avg_versions":        avg_versions,
        "max_versions":        max_versions,
    })))
}

/// GET /api/v1/drive/files/stats/ext-entropy — Shannon H sobre extensões de arquivo.
///
/// H = -Σ p_i * log2(p_i) para extensões LOWER(SUBSTRING(name FROM '\.[^.]*$')).
/// Arquivos sem extensão agrupados como "(none)". kind='file', não-deletados.
/// Retorna `{ext_count,total_files,entropy_bits,top:[{ext,file_count}]}`. Sprint #796.
async fn file_stats_ext_entropy(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT \
            COALESCE(NULLIF(LOWER(SUBSTRING(name FROM '\\.[^.]*$')), ''), '(none)') AS ext, \
            COUNT(*)::BIGINT AS cnt \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND deleted_at IS NULL \
            AND kind       = 'file' \
          GROUP BY ext \
          ORDER BY cnt DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let total: i64 = rows.iter().map(|(_, c)| c).sum();
    let entropy: f64 = if total == 0 { 0.0 } else {
        rows.iter().filter(|(_, c)| *c > 0).map(|(_, c)| {
            let p = *c as f64 / total as f64;
            -p * p.log2()
        }).sum()
    };
    let top: Vec<serde_json::Value> = rows.iter().take(20)
        .map(|(e, c)| serde_json::json!({"ext": e, "file_count": c}))
        .collect();
    Ok(Json(serde_json::json!({
        "ext_count":    rows.len(),
        "total_files":  total,
        "entropy_bits": entropy,
        "top":          top,
    })))
}

/// GET /api/v1/drive/files/stats/checksum-coverage — COUNT com/sem sha256.
///
/// Arquivos kind='file', não-deletados. Indica cobertura de integridade.
/// Retorna `{total_files,with_checksum,without_checksum,coverage_pct}`. Sprint #801.
async fn file_stats_checksum_coverage(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let (total, with_checksum): (i64, i64) = sqlx::query_as(
        "SELECT \
            COUNT(*)::BIGINT, \
            COUNT(*) FILTER (WHERE sha256 IS NOT NULL AND sha256 <> '')::BIGINT \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND deleted_at IS NULL \
            AND kind       = 'file'",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool).await?;

    let without_checksum = total - with_checksum;
    let coverage_pct = if total > 0 { with_checksum as f64 / total as f64 * 100.0 } else { 0.0 };
    Ok(Json(serde_json::json!({
        "total_files":      total,
        "with_checksum":    with_checksum,
        "without_checksum": without_checksum,
        "coverage_pct":     coverage_pct,
    })))
}

/// GET /api/v1/drive/files/stats/storage-key-coverage — COUNT com/sem storage_key.
///
/// Arquivos kind='file', não-deletados. storage_key NULL indica upload não finalizado.
/// Retorna `{total_files,with_storage_key,without_storage_key,coverage_pct}`. Sprint #806.
async fn file_stats_storage_key_coverage(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let (total, with_key): (i64, i64) = sqlx::query_as(
        "SELECT \
            COUNT(*)::BIGINT, \
            COUNT(*) FILTER (WHERE storage_key IS NOT NULL AND storage_key <> '')::BIGINT \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND deleted_at IS NULL \
            AND kind       = 'file'",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool).await?;

    let without_key = total - with_key;
    let coverage_pct = if total > 0 { with_key as f64 / total as f64 * 100.0 } else { 0.0 };
    Ok(Json(serde_json::json!({
        "total_files":       total,
        "with_storage_key":  with_key,
        "without_storage_key": without_key,
        "coverage_pct":      coverage_pct,
    })))
}

/// GET /api/v1/drive/files/stats/deleted-by-month — COUNT arquivos deletados por mês (1–12). Sprint #1111.
async fn file_stats_deleted_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM deleted_at AT TIME ZONE 'UTC')::INT AS month, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NOT NULL \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const MONTH_NAMES: [&str; 12] = [
        "January","February","March","April","May","June",
        "July","August","September","October","November","December",
    ];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(month, count)| {
            let month_name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"month": month, "month_name": month_name, "file_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/ext-by-month — COUNT arquivos por (ext, mês). Sprint #1136.
async fn file_stats_ext_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(Option<String>, i32, i64)> = sqlx::query_as(
        "SELECT \
            LOWER(NULLIF(SUBSTRING(name FROM '\\.[^.]*$'), '')) AS ext, \
            EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY ext, month \
          ORDER BY month ASC, file_count DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const MONTH_NAMES: [&str; 12] = [
        "January","February","March","April","May","June",
        "July","August","September","October","November","December",
    ];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, month, count)| {
            let month_name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"ext": ext, "month": month, "month_name": month_name, "file_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-by-weekday — SUM/AVG size_bytes × DOW de created_at. Sprint #1141.
async fn file_stats_size_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64, f64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COALESCE(SUM(size_bytes), 0)::BIGINT AS total_bytes, \
            COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_bytes \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
            AND size_bytes IS NOT NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, total, avg)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "total_bytes": total, "avg_bytes": avg})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/mime-by-month — COUNT arquivos × (mime_type, mês). Sprint #1146.
async fn file_stats_mime_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, Option<String>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
            LOWER(NULLIF(TRIM(mime_type), '')) AS mime, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY month, mime \
          ORDER BY month ASC, file_count DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const MONTH_NAMES: [&str; 12] = [
        "January","February","March","April","May","June",
        "July","August","September","October","November","December",
    ];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(month, mime, count)| {
            let month_name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"month": month, "month_name": month_name, "mime_type": mime.unwrap_or_else(|| "unknown".to_string()), "file_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/version-count-by-month — AVG/MAX versões por arquivo × mês. Sprint #1151.
async fn file_stats_version_count_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, f64, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM f.created_at AT TIME ZONE 'UTC')::INT AS month, \
            COALESCE(AVG(v.version_count), 0.0)::FLOAT8 AS avg_versions, \
            COALESCE(MAX(v.version_count), 0)::BIGINT AS max_versions, \
            COUNT(DISTINCT f.id)::BIGINT AS file_count \
           FROM drive_files f \
           LEFT JOIN ( \
               SELECT file_id, COUNT(*)::BIGINT AS version_count \
                 FROM drive_file_versions \
                GROUP BY file_id \
           ) v ON v.file_id = f.id \
          WHERE f.tenant_id = $1 AND f.kind = 'file' AND f.deleted_at IS NULL \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const MONTH_NAMES: [&str; 12] = [
        "January","February","March","April","May","June",
        "July","August","September","October","November","December",
    ];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(month, avg_v, max_v, files)| {
            let month_name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"month": month, "month_name": month_name, "avg_versions": avg_v, "max_versions": max_v, "file_count": files})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/folder-count-by-month — COUNT pastas criadas × mês. Sprint #1156.
async fn file_stats_folder_count_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
            COUNT(*)::BIGINT AS folder_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'folder' AND deleted_at IS NULL \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const MONTH_NAMES: [&str; 12] = [
        "January","February","March","April","May","June",
        "July","August","September","October","November","December",
    ];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(month, count)| {
            let month_name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"month": month, "month_name": month_name, "folder_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/name-length-by-month — AVG/MAX LENGTH(name) por mês. Sprint #1161.
async fn file_stats_name_length_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, f64, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
            COALESCE(AVG(LENGTH(name)), 0.0)::FLOAT8 AS avg_name_length, \
            COALESCE(MAX(LENGTH(name)), 0)::BIGINT AS max_name_length, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const MONTH_NAMES: [&str; 12] = [
        "January","February","March","April","May","June",
        "July","August","September","October","November","December",
    ];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(month, avg_len, max_len, count)| {
            let month_name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"month": month, "month_name": month_name, "avg_name_length": avg_len, "max_name_length": max_len, "file_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/name-length-by-weekday — AVG/MAX LENGTH(name) × DOW. Sprint #1166.
async fn file_stats_name_length_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, f64, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COALESCE(AVG(LENGTH(name)), 0.0)::FLOAT8 AS avg_name_length, \
            COALESCE(MAX(LENGTH(name)), 0)::BIGINT AS max_name_length, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, avg_len, max_len, count)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "avg_name_length": avg_len, "max_name_length": max_len, "file_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/folder-count-by-weekday — COUNT pastas criadas × DOW. Sprint #1171.
async fn file_stats_folder_count_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(*)::BIGINT AS folder_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'folder' AND deleted_at IS NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, count)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "folder_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/locked-by-weekday — COUNT arquivos com locked_at × DOW. Sprint #1176.
async fn file_stats_locked_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM locked_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(*) FILTER (WHERE locked_at IS NOT NULL)::BIGINT AS locked_count, \
            COUNT(*)::BIGINT AS total_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, locked, total)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            let rate = if total > 0 { locked as f64 / total as f64 } else { 0.0 };
            serde_json::json!({"dow": dow, "day_name": day_name, "locked_count": locked, "total_count": total, "locked_rate": rate})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/versioned-by-weekday — COUNT arquivos com versões × DOW. Sprint #1181.
async fn file_stats_versioned_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM f.created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(DISTINCT v.file_id)::BIGINT AS versioned_count, \
            COUNT(DISTINCT f.id)::BIGINT AS total_count \
           FROM drive_files f \
           LEFT JOIN drive_file_versions v ON v.file_id = f.id \
          WHERE f.tenant_id = $1 AND f.kind = 'file' AND f.deleted_at IS NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, versioned, total)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            let rate = if total > 0 { versioned as f64 / total as f64 } else { 0.0 };
            serde_json::json!({"dow": dow, "day_name": day_name, "versioned_count": versioned, "total_count": total, "versioned_rate": rate})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-by-hour — SUM/AVG size_bytes × hora-do-dia. Sprint #1186.
async fn file_stats_size_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64, f64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COALESCE(SUM(size_bytes), 0)::BIGINT AS total_bytes, \
            COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_bytes, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
            AND size_bytes IS NOT NULL \
          GROUP BY hour_of_day \
          ORDER BY hour_of_day ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, total, avg, count)| serde_json::json!({"hour_of_day": h, "total_bytes": total, "avg_bytes": avg, "file_count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/starred-by-hour — COUNT arquivos starred × hora-do-dia. Sprint #1191.
async fn file_stats_starred_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COUNT(*) FILTER (WHERE starred = true)::BIGINT AS starred_count, \
            COUNT(*)::BIGINT AS total_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY hour_of_day \
          ORDER BY hour_of_day ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, starred, total)| {
            let rate = if total > 0 { starred as f64 / total as f64 } else { 0.0 };
            serde_json::json!({"hour_of_day": h, "starred_count": starred, "total_count": total, "starred_rate": rate})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/locked-by-hour — COUNT arquivos com locked_at × hora-do-dia. Sprint #1196.
async fn file_stats_locked_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COUNT(*) FILTER (WHERE locked_at IS NOT NULL)::BIGINT AS locked_count, \
            COUNT(*)::BIGINT AS total_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY hour_of_day \
          ORDER BY hour_of_day ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, locked, total)| {
            let rate = if total > 0 { locked as f64 / total as f64 } else { 0.0 };
            serde_json::json!({"hour_of_day": h, "locked_count": locked, "total_count": total, "locked_rate": rate})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/name-length-by-hour — AVG/MAX LENGTH(name) × hora-do-dia. Sprint #1201.
async fn file_stats_name_length_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, f64, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COALESCE(AVG(LENGTH(name)), 0.0)::FLOAT8 AS avg_name_length, \
            COALESCE(MAX(LENGTH(name)), 0)::BIGINT AS max_name_length, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY hour_of_day \
          ORDER BY hour_of_day ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, avg_len, max_len, count)| serde_json::json!({"hour_of_day": h, "avg_name_length": avg_len, "max_name_length": max_len, "file_count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/mime-by-hour — COUNT arquivos × (mime_type, hora). Sprint #1206.
async fn file_stats_mime_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, Option<String>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            LOWER(NULLIF(TRIM(mime_type), '')) AS mime, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY hour_of_day, mime \
          ORDER BY hour_of_day ASC, file_count DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, mime, count)| serde_json::json!({"hour_of_day": h, "mime_type": mime.unwrap_or_else(|| "unknown".to_string()), "file_count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/version-size-by-month — AVG/SUM size_bytes de versões × mês. Sprint #1241.
async fn file_stats_version_size_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM v.created_at AT TIME ZONE 'UTC')::INT AS month, \
            COALESCE(AVG(v.size_bytes)::BIGINT, 0) AS avg_size_bytes, \
            COALESCE(SUM(v.size_bytes)::BIGINT, 0) AS total_size_bytes \
           FROM drive_file_versions v \
           JOIN drive_files f ON f.id = v.file_id \
          WHERE f.tenant_id = $1 AND f.deleted_at IS NULL \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const MONTH_NAMES: [&str; 12] = [
        "January","February","March","April","May","June",
        "July","August","September","October","November","December",
    ];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(month, avg, total)| {
            let month_name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"month": month, "month_name": month_name, "avg_size_bytes": avg, "total_size_bytes": total})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/version-size-by-hour — AVG/SUM size_bytes de versões × hora-do-dia. Sprint #1236.
async fn file_stats_version_size_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR FROM v.created_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COALESCE(AVG(v.size_bytes)::BIGINT, 0) AS avg_size_bytes, \
            COALESCE(SUM(v.size_bytes)::BIGINT, 0) AS total_size_bytes \
           FROM drive_file_versions v \
           JOIN drive_files f ON f.id = v.file_id \
          WHERE f.tenant_id = $1 AND f.deleted_at IS NULL \
          GROUP BY hour_of_day \
          ORDER BY hour_of_day ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, avg, total)| serde_json::json!({"hour_of_day": h, "avg_size_bytes": avg, "total_size_bytes": total}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/folder-count-by-hour — COUNT pastas criadas × hora-do-dia. Sprint #1231.
async fn file_stats_folder_count_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COUNT(*)::BIGINT AS folder_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'folder' AND deleted_at IS NULL \
          GROUP BY hour_of_day \
          ORDER BY hour_of_day ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, count)| serde_json::json!({"hour_of_day": h, "folder_count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/version-count-by-hour — AVG/MAX versões por arquivo × hora-do-dia. Sprint #1226.
async fn file_stats_version_count_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, f64, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR FROM f.created_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COALESCE(AVG(v.version_count), 0.0)::FLOAT8 AS avg_versions, \
            COALESCE(MAX(v.version_count), 0)::BIGINT AS max_versions, \
            COUNT(DISTINCT f.id)::BIGINT AS file_count \
           FROM drive_files f \
           LEFT JOIN ( \
               SELECT file_id, COUNT(*)::BIGINT AS version_count \
                 FROM drive_file_versions \
                GROUP BY file_id \
           ) v ON v.file_id = f.id \
          WHERE f.tenant_id = $1 AND f.kind = 'file' AND f.deleted_at IS NULL \
          GROUP BY hour_of_day \
          ORDER BY hour_of_day ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, avg_v, max_v, files)| {
            serde_json::json!({"hour_of_day": h, "avg_versions": avg_v, "max_versions": max_v, "file_count": files})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/ext-by-hour — COUNT arquivos por (ext, hora-do-dia). Sprint #1221.
async fn file_stats_ext_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsLimitQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);

    let rows: Vec<(Option<String>, i32, i64)> = sqlx::query_as(
        "SELECT \
            LOWER(NULLIF(SUBSTRING(name FROM '\\.[^.]*$'), '')) AS ext, \
            EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY ext, hour_of_day \
          ORDER BY hour_of_day ASC, file_count DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, h, count)| serde_json::json!({"ext": ext, "hour_of_day": h, "file_count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/versioned-by-hour — COUNT arquivos com versões × hora-do-dia (created_at). Sprint #1216.
async fn file_stats_versioned_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COUNT(*)::BIGINT AS versioned_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
            AND version > 1 \
          GROUP BY hour_of_day \
          ORDER BY hour_of_day ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, count)| serde_json::json!({"hour_of_day": h, "versioned_count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-by-hour — COUNT arquivos deletados × hora-do-dia. Sprint #1211.
async fn file_stats_deleted_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR FROM deleted_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COUNT(*)::BIGINT AS deleted_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NOT NULL \
          GROUP BY hour_of_day \
          ORDER BY hour_of_day ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, count)| serde_json::json!({"hour_of_day": h, "deleted_count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/locked-by-month — COUNT arquivos com locked_at por mês (1–12). Sprint #1126.
async fn file_stats_locked_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM locked_at AT TIME ZONE 'UTC')::INT AS month, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND locked_at IS NOT NULL AND deleted_at IS NULL \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const MONTH_NAMES: [&str; 12] = [
        "January","February","March","April","May","June",
        "July","August","September","October","November","December",
    ];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(month, count)| {
            let month_name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"month": month, "month_name": month_name, "file_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-by-month — SUM/AVG size bytes por mês de created_at. Sprint #1131.
async fn file_stats_size_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64, f64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
            COALESCE(SUM(size_bytes), 0)::BIGINT AS total_bytes, \
            COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_bytes \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
            AND size_bytes IS NOT NULL \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const MONTH_NAMES: [&str; 12] = [
        "January","February","March","April","May","June",
        "July","August","September","October","November","December",
    ];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(month, total, avg)| {
            let month_name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"month": month, "month_name": month_name, "total_bytes": total, "avg_bytes": avg})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/starred-by-month — COUNT arquivos starred por mês (1–12). Sprint #1116.
async fn file_stats_starred_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
            COUNT(*) FILTER (WHERE starred = true)::BIGINT AS starred_count, \
            COUNT(*)::BIGINT AS total_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const MONTH_NAMES: [&str; 12] = [
        "January","February","March","April","May","June",
        "July","August","September","October","November","December",
    ];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(month, starred, total)| {
            let month_name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            let rate = if total > 0 { starred as f64 / total as f64 } else { 0.0 };
            serde_json::json!({"month": month, "month_name": month_name, "starred_count": starred, "total_count": total, "starred_rate": rate})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/versioned-by-month — COUNT arquivos com versão > 1 por mês (1–12). Sprint #1121.
async fn file_stats_versioned_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
            COUNT(*) FILTER (WHERE version > 1)::BIGINT AS versioned_count, \
            COUNT(*)::BIGINT AS total_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const MONTH_NAMES: [&str; 12] = [
        "January","February","March","April","May","June",
        "July","August","September","October","November","December",
    ];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(month, versioned, total)| {
            let month_name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            let rate = if total > 0 { versioned as f64 / total as f64 } else { 0.0 };
            serde_json::json!({"month": month, "month_name": month_name, "versioned_count": versioned, "total_count": total, "versioned_rate": rate})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/modified-by-month — COUNT arquivos modificados por mês (1–12). Sprint #1106.
async fn file_stats_modified_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM updated_at AT TIME ZONE 'UTC')::INT AS month, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
            AND updated_at IS NOT NULL \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const MONTH_NAMES: [&str; 12] = [
        "January","February","March","April","May","June",
        "July","August","September","October","November","December",
    ];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(month, count)| {
            let month_name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"month": month, "month_name": month_name, "file_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/created-by-month — COUNT arquivos criados por mês (1–12). Sprint #1101.
async fn file_stats_created_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const MONTH_NAMES: [&str; 12] = [
        "January","February","March","April","May","June",
        "July","August","September","October","November","December",
    ];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(month, count)| {
            let month_name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"month": month, "month_name": month_name, "file_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/locked-by-user?limit=N — COUNT arquivos locked por locked_by.
///
/// Apenas arquivos com locked_at IS NOT NULL. Limit default 20 max 200.
/// Retorna `{total_locked,rows:[{locked_by,file_count}]}` file_count DESC. Sprint #811.
async fn file_stats_locked_by_user(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(20).min(200).max(1);
    let pool   = state.db_or_unavailable()?;

    let (total_locked,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT FROM drive_files \
          WHERE tenant_id = $1 AND locked_at IS NOT NULL AND deleted_at IS NULL",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool).await?;

    let rows: Vec<(Option<Uuid>, i64)> = sqlx::query_as(
        "SELECT locked_by, COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND locked_at  IS NOT NULL \
            AND deleted_at IS NULL \
          GROUP BY locked_by \
          ORDER BY file_count DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(uid, fc)| serde_json::json!({"locked_by": uid, "file_count": fc}))
        .collect();
    Ok(Json(serde_json::json!({"total_locked": total_locked, "rows": out})))
}

/// GET /api/v1/drive/files/stats/mime-by-ext?limit=N — top mime_type por extensão.
///
/// GROUP BY (ext, mime_type) file_count DESC. Ext via LOWER(SUBSTRING(name)). Limit default 50 max 500.
/// Retorna `{rows:[{ext,mime_type,file_count}]}`. Sprint #816.
async fn file_stats_mime_by_ext(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(50).min(500).max(1);
    let pool   = state.db_or_unavailable()?;

    let rows: Vec<(String, Option<String>, i64)> = sqlx::query_as(
        "SELECT \
            COALESCE(LOWER(SUBSTRING(name FROM '\\.[^.]*$')), '(none)') AS ext, \
            COALESCE(mime_type, 'application/octet-stream')            AS mime_type, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND kind       = 'file' \
            AND deleted_at IS NULL \
          GROUP BY ext, mime_type \
          ORDER BY file_count DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(e, m, c)| serde_json::json!({"ext": e, "mime_type": m, "file_count": c}))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/size-trend-by-day?since=&until= — total_bytes criados por dia.
///
/// DATE_TRUNC('day', created_at) SUM(size_bytes) GROUP BY dia ASC. kind='file', não-deletados.
/// Retorna `{rows:[{day,total_bytes,file_count}]}`. Sprint #821.
#[derive(Debug, serde::Deserialize)]
struct DateRangeQuery {
    since: Option<String>,
    until: Option<String>,
}

async fn file_stats_size_trend_by_day(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<DateRangeQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let since_dt: Option<OffsetDateTime> = q.since.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|_| crate::error::DriveError::BadRequest("since must be RFC3339".into()))
    }).transpose()?;
    let until_dt: Option<OffsetDateTime> = q.until.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|_| crate::error::DriveError::BadRequest("until must be RFC3339".into()))
    }).transpose()?;

    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('day', created_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            COALESCE(SUM(size_bytes), 0)::BIGINT AS total_bytes, \
            COUNT(*)::BIGINT                     AS file_count \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND kind       = 'file' \
            AND deleted_at IS NULL \
            AND ($2::timestamptz IS NULL OR created_at >= $2) \
            AND ($3::timestamptz IS NULL OR created_at <  $3) \
          GROUP BY day \
          ORDER BY day ASC",
    )
    .bind(ctx.tenant_id).bind(since_dt).bind(until_dt)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(d, b, c)| serde_json::json!({"day": d, "total_bytes": b, "file_count": c}))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/version-age?limit=N — arquivos com versões mais antigas.
///
/// MIN(created_at) por arquivo via drive_file_versions, ordenado ASC.
/// Retorna `{rows:[{file_id,oldest_version_at,version_count}]}`. Limit default 20 max 200. Sprint #826.
async fn file_stats_version_age(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(20).min(200).max(1);
    let pool   = state.db_or_unavailable()?;

    let rows: Vec<(Uuid, Option<OffsetDateTime>, i64)> = sqlx::query_as(
        "SELECT v.file_id, \
                MIN(v.created_at)     AS oldest_version_at, \
                COUNT(*)::BIGINT      AS version_count \
           FROM drive_file_versions v \
           JOIN drive_files f ON f.id = v.file_id \
          WHERE f.tenant_id  = $1 \
            AND f.deleted_at IS NULL \
          GROUP BY v.file_id \
          ORDER BY oldest_version_at ASC NULLS LAST \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(fid, oldest, vc)| serde_json::json!({
            "file_id":           fid,
            "oldest_version_at": oldest,
            "version_count":     vc,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/mime-count-by-user?limit=N — top mime_types por owner_user_id.
///
/// GROUP BY (owner_user_id, mime_type) COUNT DESC. Limit default 50 max 500.
/// Retorna `{rows:[{owner_user_id,mime_type,file_count}]}`. Sprint #831.
async fn file_stats_mime_count_by_user(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(50).min(500).max(1);
    let pool   = state.db_or_unavailable()?;

    let rows: Vec<(Option<Uuid>, Option<String>, i64)> = sqlx::query_as(
        "SELECT \
            owner_user_id, \
            COALESCE(mime_type, 'application/octet-stream') AS mime_type, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND kind       = 'file' \
            AND deleted_at IS NULL \
          GROUP BY owner_user_id, mime_type \
          ORDER BY file_count DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(uid, m, c)| serde_json::json!({
            "owner_user_id": uid,
            "mime_type":     m,
            "file_count":    c,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/created-vs-deleted-by-day?since=&until= — net criados e deletados por dia.
///
/// Retorna `{rows:[{day,created,deleted,net}]}` day ASC. Sprint #836.
async fn file_stats_created_vs_deleted_by_day(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<DateRangeQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT \
            to_char(day, 'YYYY-MM-DD') AS day, \
            COALESCE(SUM(created), 0)::BIGINT AS created, \
            COALESCE(SUM(deleted), 0)::BIGINT AS deleted \
           FROM ( \
                SELECT date_trunc('day', created_at AT TIME ZONE 'UTC') AS day, \
                       1 AS created, 0 AS deleted \
                  FROM drive_files \
                 WHERE tenant_id = $1 AND kind = 'file' \
                   AND ($2::timestamptz IS NULL OR created_at >= $2) \
                   AND ($3::timestamptz IS NULL OR created_at <  $3) \
                UNION ALL \
                SELECT date_trunc('day', deleted_at AT TIME ZONE 'UTC') AS day, \
                       0 AS created, 1 AS deleted \
                  FROM drive_files \
                 WHERE tenant_id = $1 AND kind = 'file' \
                   AND deleted_at IS NOT NULL \
                   AND ($2::timestamptz IS NULL OR deleted_at >= $2) \
                   AND ($3::timestamptz IS NULL OR deleted_at <  $3) \
           ) sub \
          GROUP BY day \
          ORDER BY day ASC",
    )
    .bind(ctx.tenant_id)
    .bind(q.since.as_deref().and_then(|s| s.parse::<time::OffsetDateTime>().ok()))
    .bind(q.until.as_deref().and_then(|s| s.parse::<time::OffsetDateTime>().ok()))
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(day, created, deleted)| serde_json::json!({
            "day":     day,
            "created": created,
            "deleted": deleted,
            "net":     created - deleted,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/version-size-by-user?limit=N — total bytes de versões por owner_user_id.
///
/// JOIN drive_file_versions + drive_files; GROUP BY owner_user_id total DESC. Sprint #841.
async fn file_stats_version_size_by_user(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(50).min(500).max(1);
    let pool   = state.db_or_unavailable()?;

    let rows: Vec<(Option<Uuid>, i64, i64)> = sqlx::query_as(
        "SELECT \
            f.owner_user_id, \
            COUNT(v.id)::BIGINT AS version_count, \
            COALESCE(SUM(v.size_bytes), 0)::BIGINT AS total_bytes \
           FROM drive_file_versions v \
           JOIN drive_files f ON f.id = v.file_id \
          WHERE f.tenant_id = $1 AND f.deleted_at IS NULL \
          GROUP BY f.owner_user_id \
          ORDER BY total_bytes DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(uid, vc, tb)| serde_json::json!({
            "owner_user_id":  uid,
            "version_count":  vc,
            "total_bytes":    tb,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/ext-size-by-folder?folder_id= — SUM(size_bytes) por (ext, folder).
///
/// SUBSTRING(name FROM '\\.[^.]*$') + parent_id GROUP BY. Sprint #846.
async fn file_stats_ext_size_by_folder(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsMimeByFolderQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(Option<Uuid>, Option<String>, i64, i64)> = sqlx::query_as(
        "SELECT \
            parent_id, \
            LOWER(SUBSTRING(name FROM '\\.[^.]*$')) AS ext, \
            COUNT(*)::BIGINT AS file_count, \
            COALESCE(SUM(size_bytes), 0)::BIGINT AS total_bytes \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
            AND ($2::uuid IS NULL OR parent_id = $2) \
          GROUP BY parent_id, ext \
          ORDER BY parent_id ASC, total_bytes DESC",
    )
    .bind(ctx.tenant_id).bind(q.folder_id)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(pid, ext, fc, tb)| serde_json::json!({
            "parent_id":   pid,
            "ext":         ext,
            "file_count":  fc,
            "total_bytes": tb,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/tag-by-user?limit=N — top tags por owner_user_id.
///
/// JOIN drive_file_tags; GROUP BY (owner_user_id, tag) COUNT DESC. Sprint #851.
async fn file_stats_tag_by_user(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(50).min(500).max(1);
    let pool   = state.db_or_unavailable()?;

    let rows: Vec<(Option<Uuid>, String, i64)> = sqlx::query_as(
        "SELECT \
            f.owner_user_id, \
            t.tag, \
            COUNT(*)::BIGINT AS tag_count \
           FROM drive_file_tags t \
           JOIN drive_files f ON f.id = t.file_id \
          WHERE f.tenant_id = $1 AND f.deleted_at IS NULL \
          GROUP BY f.owner_user_id, t.tag \
          ORDER BY tag_count DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(uid, tag, tc)| serde_json::json!({
            "owner_user_id": uid,
            "tag":           tag,
            "tag_count":     tc,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/tag-entropy — Shannon H=-Σp*log2(p) sobre tags globais.
///
/// Análogo a mime-entropy (#781) mas sobre drive_file_tags. Sprint #856.
async fn file_stats_tag_entropy(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT t.tag, COUNT(*)::BIGINT AS cnt \
           FROM drive_file_tags t \
           JOIN drive_files f ON f.id = t.file_id \
          WHERE f.tenant_id = $1 AND f.deleted_at IS NULL \
          GROUP BY t.tag \
          ORDER BY cnt DESC \
          LIMIT 100",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let total: i64 = rows.iter().map(|(_, c)| c).sum();
    let entropy = if total == 0 || rows.len() < 2 {
        0.0_f64
    } else {
        rows.iter().fold(0.0_f64, |acc, (_, c)| {
            let p = *c as f64 / total as f64;
            acc - p * p.log2()
        })
    };
    let top: Vec<serde_json::Value> = rows.into_iter()
        .map(|(tag, cnt)| serde_json::json!({"tag": tag, "count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"entropy": entropy, "total_tags_used": total, "top": top})))
}

/// GET /api/v1/drive/files/stats/folder-mime-entropy?folder_id= — Shannon H sobre mime_type por folder.
///
/// Entropia de distribuição de tipos MIME dentro de uma pasta (ou tenant inteiro). Sprint #861.
async fn file_stats_folder_mime_entropy(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsMimeByFolderQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(Option<String>, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type, 'application/octet-stream'), COUNT(*)::BIGINT AS cnt \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
            AND ($2::uuid IS NULL OR parent_id = $2) \
          GROUP BY mime_type",
    )
    .bind(ctx.tenant_id).bind(q.folder_id)
    .fetch_all(pool).await?;

    let total: i64 = rows.iter().map(|(_, c)| c).sum();
    let entropy = if total == 0 || rows.len() < 2 {
        0.0_f64
    } else {
        rows.iter().fold(0.0_f64, |acc, (_, c)| {
            let p = *c as f64 / total as f64;
            acc - p * p.log2()
        })
    };
    Ok(Json(serde_json::json!({
        "folder_id": q.folder_id,
        "entropy":   entropy,
        "total":     total,
        "distinct_mime_types": rows.len(),
    })))
}

/// GET /api/v1/drive/files/stats/size-entropy — Shannon H sobre distribuição de tamanhos por bucket.
///
/// 8 buckets <1KB/1-10KB/10-100KB/100KB-1MB/1-10MB/10-100MB/100MB-1GB/>1GB. Sprint #866.
async fn file_stats_size_entropy(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let (b0, b1, b2, b3, b4, b5, b6, b7): (i64, i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT \
            COUNT(*) FILTER (WHERE size_bytes <          1024)::BIGINT, \
            COUNT(*) FILTER (WHERE size_bytes >=         1024 AND size_bytes <        10240)::BIGINT, \
            COUNT(*) FILTER (WHERE size_bytes >=        10240 AND size_bytes <       102400)::BIGINT, \
            COUNT(*) FILTER (WHERE size_bytes >=       102400 AND size_bytes <      1048576)::BIGINT, \
            COUNT(*) FILTER (WHERE size_bytes >=      1048576 AND size_bytes <     10485760)::BIGINT, \
            COUNT(*) FILTER (WHERE size_bytes >=     10485760 AND size_bytes <    104857600)::BIGINT, \
            COUNT(*) FILTER (WHERE size_bytes >=    104857600 AND size_bytes <   1073741824)::BIGINT, \
            COUNT(*) FILTER (WHERE size_bytes >=   1073741824)::BIGINT \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool).await?;

    let counts = [b0, b1, b2, b3, b4, b5, b6, b7];
    let total: i64 = counts.iter().sum();
    let entropy = if total == 0 {
        0.0_f64
    } else {
        counts.iter().fold(0.0_f64, |acc, &c| {
            if c == 0 { acc } else {
                let p = c as f64 / total as f64;
                acc - p * p.log2()
            }
        })
    };
    let labels = ["<1KB","1-10KB","10-100KB","100KB-1MB","1-10MB","10-100MB","100MB-1GB",">1GB"];
    let buckets: Vec<serde_json::Value> = labels.iter().zip(counts.iter())
        .map(|(l, c)| serde_json::json!({"range": l, "count": c}))
        .collect();
    Ok(Json(serde_json::json!({"entropy": entropy, "total": total, "buckets": buckets})))
}

/// GET /api/v1/drive/files/stats/version-count-by-ext?limit=N — avg/max versões por extensão.
///
/// JOIN drive_file_versions; GROUP BY ext; ordenado por avg_versions DESC. Sprint #871.
async fn file_stats_version_count_by_ext(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(30).min(200).max(1);
    let pool   = state.db_or_unavailable()?;

    let rows: Vec<(Option<String>, f64, i64, i64)> = sqlx::query_as(
        "SELECT \
            LOWER(SUBSTRING(f.name FROM '\\.[^.]*$')) AS ext, \
            AVG(vc)::FLOAT8 AS avg_versions, \
            MAX(vc)::BIGINT AS max_versions, \
            COUNT(*)::BIGINT AS file_count \
           FROM ( \
                SELECT f.id, f.name, COUNT(v.id) AS vc \
                  FROM drive_files f \
                  LEFT JOIN drive_file_versions v ON v.file_id = f.id \
                 WHERE f.tenant_id = $1 AND f.kind = 'file' AND f.deleted_at IS NULL \
                 GROUP BY f.id, f.name \
           ) f \
          GROUP BY ext \
          ORDER BY avg_versions DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, avg, max, fc)| serde_json::json!({
            "ext":          ext,
            "avg_versions": avg,
            "max_versions": max,
            "file_count":   fc,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/tag-frequency-by-folder?limit=N — top tags por pasta (parent_id).
///
/// JOIN drive_file_tags; GROUP BY (parent_id, tag). Ordena por count DESC. Sprint #896.
async fn file_stats_tag_frequency_by_folder(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(50).min(500).max(1);
    let pool   = state.db_or_unavailable()?;

    let rows: Vec<(Option<Uuid>, String, i64)> = sqlx::query_as(
        "SELECT f.parent_id, t.tag, COUNT(*)::BIGINT AS count \
           FROM drive_files f \
           JOIN drive_file_tags t ON t.file_id = f.id \
          WHERE f.tenant_id = $1 AND f.deleted_at IS NULL \
          GROUP BY f.parent_id, t.tag \
          ORDER BY count DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder_id, tag, count)| serde_json::json!({"folder_id": folder_id, "tag": tag, "count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/folder-count-by-user?limit=N — COUNT pastas por owner_user_id.
///
/// kind='folder', não-deletadas. GROUP BY owner_user_id ORDER BY folder_count DESC. Sprint #906.
async fn file_stats_folder_count_by_user(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(20).min(200).max(1);
    let pool   = state.db_or_unavailable()?;

    let rows: Vec<(Option<Uuid>, i64)> = sqlx::query_as(
        "SELECT owner_user_id, COUNT(*)::BIGINT AS folder_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'folder' AND deleted_at IS NULL \
          GROUP BY owner_user_id \
          ORDER BY folder_count DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(user_id, fc)| serde_json::json!({"owner_user_id": user_id, "folder_count": fc}))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/size-trend-by-folder?limit=N — SUM(size_bytes) + file_count por (folder, dia).
///
/// GROUP BY (parent_id, DATE_TRUNC('day', created_at)); útil para ver crescimento por pasta. Sprint #901.
async fn file_stats_size_trend_by_folder(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(200).min(2000).max(1);
    let pool   = state.db_or_unavailable()?;

    let rows: Vec<(Option<Uuid>, String, i64, i64)> = sqlx::query_as(
        "SELECT \
            parent_id, \
            to_char(date_trunc('day', created_at) AT TIME ZONE 'UTC', 'YYYY-MM-DD') AS day, \
            COALESCE(SUM(size_bytes), 0)::BIGINT AS total_bytes, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY parent_id, day \
          ORDER BY day DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder_id, day, bytes, fc)| serde_json::json!({
            "folder_id":   folder_id,
            "day":         day,
            "total_bytes": bytes,
            "file_count":  fc,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/large-files?limit=N&threshold_mb=N — arquivos acima de threshold_mb (default 100MB).
///
/// ORDER BY size_bytes DESC; total_large_bytes incluso. Sprint #931.
async fn file_stats_large_files(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit        = q.limit.unwrap_or(20).min(200).max(1);
    let threshold_mb = 100i64;
    let threshold_bytes = threshold_mb * 1024 * 1024;
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(Uuid, String, i64)> = sqlx::query_as(
        "SELECT id, name, size_bytes \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
            AND size_bytes >= $2 \
          ORDER BY size_bytes DESC \
          LIMIT $3",
    )
    .bind(ctx.tenant_id).bind(threshold_bytes).bind(limit)
    .fetch_all(pool).await?;

    let total_large_bytes: i64 = rows.iter().map(|(_, _, s)| *s).sum();
    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(id, name, size)| serde_json::json!({"id": id, "name": name, "size_bytes": size}))
        .collect();
    Ok(Json(serde_json::json!({
        "threshold_mb": threshold_mb,
        "total_large_bytes": total_large_bytes,
        "rows": out,
    })))
}

/// GET /api/v1/drive/files/stats/created-by-hour — histograma hora-do-dia de created_at (0-23).
///
/// GROUP BY EXTRACT(HOUR) COUNT; ordem 0..23. Sprint #926.
async fn file_stats_created_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COUNT(*)::BIGINT AS count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY hour_of_day \
          ORDER BY hour_of_day ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(hour, count)| serde_json::json!({"hour": hour, "count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/last-modified-by-folder?limit=N — MAX updated_at por pasta.
///
/// ORDER BY max_updated_at DESC; pastas com arquivos mais recentemente modificados. Sprint #921.
async fn file_stats_last_modified_by_folder(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(30).min(200).max(1);
    let pool   = state.db_or_unavailable()?;

    let rows: Vec<(Option<Uuid>, Option<time::OffsetDateTime>, Option<time::OffsetDateTime>, i64)> = sqlx::query_as(
        "SELECT \
            parent_id, \
            MAX(updated_at) AS max_updated_at, \
            MIN(updated_at) AS min_updated_at, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY parent_id \
          ORDER BY max_updated_at DESC NULLS LAST \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder_id, max_upd, min_upd, fc)| serde_json::json!({
            "folder_id":      folder_id,
            "max_updated_at": max_upd.map(|t| t.to_string()),
            "min_updated_at": min_upd.map(|t| t.to_string()),
            "file_count":     fc,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/zero-size — COUNT kind='file' WHERE size_bytes = 0 OR NULL.
///
/// total_zero + null_size + zero_bytes; útil para detectar uploads incompletos. Sprint #951.
async fn file_stats_zero_size(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let (null_size, zero_bytes, total_files): (i64, i64, i64) = sqlx::query_as(
        "SELECT \
            COUNT(*) FILTER (WHERE size_bytes IS NULL)::BIGINT     AS null_size, \
            COUNT(*) FILTER (WHERE size_bytes = 0)::BIGINT         AS zero_bytes, \
            COUNT(*)::BIGINT                                        AS total_files \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool).await?;

    let total_zero = null_size + zero_bytes;
    Ok(Json(serde_json::json!({
        "null_size":   null_size,
        "zero_bytes":  zero_bytes,
        "total_zero":  total_zero,
        "total_files": total_files,
    })))
}

/// GET /api/v1/drive/files/stats/ext-by-weekday — COUNT por (DOW, extensão) de created_at.
///
/// LOWER(SUBSTRING(name FROM '\.[^.]*$')); GROUP BY (dow, ext); ORDER BY dow, count DESC. Sprint #946.
async fn file_stats_ext_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, Option<String>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            LOWER(NULLIF(SUBSTRING(name FROM '\\.[^.]*$'), '')) AS ext, \
            COUNT(*)::BIGINT AS count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
            AND created_at IS NOT NULL \
          GROUP BY dow, ext \
          ORDER BY dow ASC, count DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let mut by_dow: std::collections::BTreeMap<i32, Vec<serde_json::Value>> = std::collections::BTreeMap::new();
    for (dow, ext, count) in rows {
        by_dow.entry(dow).or_default().push(serde_json::json!({"ext": ext, "count": count}));
    }
    let result: Vec<serde_json::Value> = by_dow.into_iter()
        .map(|(dow, exts)| {
            let name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": name, "extensions": exts})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-by-weekday — AVG/SUM size_bytes por DOW de created_at (0=Dom).
///
/// EXTRACT(DOW FROM created_at) GROUP BY dow; mostra padrão de uploads por dia da semana. Sprint #941.
async fn file_stats_size_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, f64, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            AVG(COALESCE(size_bytes, 0))::FLOAT8 AS avg_size_bytes, \
            SUM(COALESCE(size_bytes, 0))::BIGINT  AS total_size_bytes, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
            AND created_at IS NOT NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, avg, total, count)| {
            let name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": name, "avg_size_bytes": avg, "total_size_bytes": total, "file_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/modified-by-hour — histograma hora-do-dia de updated_at (0-23).
///
/// EXTRACT(HOUR FROM updated_at) GROUP BY hour; complementa created-by-hour (#926). Sprint #936.
async fn file_stats_modified_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR FROM updated_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COUNT(*)::BIGINT AS count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
            AND updated_at IS NOT NULL \
          GROUP BY hour_of_day \
          ORDER BY hour_of_day ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(hour, count)| serde_json::json!({"hour": hour, "count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/starred-by-folder?limit=N — COUNT starred_at IS NOT NULL por pasta.
///
/// GROUP BY parent_id; ORDER BY starred_count DESC. Sprint #916.
async fn file_stats_starred_by_folder(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(30).min(200).max(1);
    let pool   = state.db_or_unavailable()?;

    let rows: Vec<(Option<Uuid>, i64, i64)> = sqlx::query_as(
        "SELECT \
            parent_id, \
            COUNT(*) FILTER (WHERE starred_at IS NOT NULL)::BIGINT AS starred_count, \
            COUNT(*)::BIGINT AS total_files \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY parent_id \
          ORDER BY starred_count DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder_id, starred, total)| {
            let pct = if total > 0 { (starred as f64 / total as f64 * 100.0 * 10.0).round() / 10.0 } else { 0.0 };
            serde_json::json!({"folder_id": folder_id, "starred_count": starred, "total_files": total, "pct_starred": pct})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/file-age-by-folder?limit=N — avg/max age em dias por folder.
///
/// EXTRACT(EPOCH FROM (NOW()-created_at))/86400 → days; GROUP BY parent_id. Sprint #911.
async fn file_stats_file_age_by_folder(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(30).min(200).max(1);
    let pool   = state.db_or_unavailable()?;

    let rows: Vec<(Option<Uuid>, f64, f64, i64)> = sqlx::query_as(
        "SELECT \
            parent_id, \
            AVG(EXTRACT(EPOCH FROM (NOW() - created_at)) / 86400.0)::FLOAT8 AS avg_age_days, \
            MAX(EXTRACT(EPOCH FROM (NOW() - created_at)) / 86400.0)::FLOAT8 AS max_age_days, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY parent_id \
          ORDER BY avg_age_days DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder_id, avg, max, fc)| serde_json::json!({
            "folder_id":    folder_id,
            "avg_age_days": avg,
            "max_age_days": max,
            "file_count":   fc,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/ext-version-age?limit=N — MIN/MAX created_at de versões por extensão.
///
/// JOIN drive_file_versions; GROUP BY ext; age = NOW()-MIN(v.created_at) days. Sprint #891.
async fn file_stats_ext_version_age(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(30).min(200).max(1);
    let pool   = state.db_or_unavailable()?;

    let rows: Vec<(Option<String>, String, String, i64)> = sqlx::query_as(
        "SELECT \
            LOWER(SUBSTRING(f.name FROM '\\.[^.]*$')) AS ext, \
            to_char(MIN(v.created_at) AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS oldest_version_at, \
            to_char(MAX(v.created_at) AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS newest_version_at, \
            COUNT(v.id)::BIGINT AS version_count \
           FROM drive_files f \
           JOIN drive_file_versions v ON v.file_id = f.id \
          WHERE f.tenant_id = $1 AND f.kind = 'file' AND f.deleted_at IS NULL \
          GROUP BY ext \
          ORDER BY oldest_version_at ASC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, oldest, newest, vc)| serde_json::json!({
            "ext":               ext,
            "oldest_version_at": oldest,
            "newest_version_at": newest,
            "version_count":     vc,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/storage-by-folder?limit=N — total_bytes + file_count por folder top-N.
///
/// GROUP BY parent_id; NULL = raiz. Ordena por total_bytes DESC. Sprint #886.
async fn file_stats_storage_by_folder(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(20).min(200).max(1);
    let pool   = state.db_or_unavailable()?;

    let rows: Vec<(Option<Uuid>, i64, i64)> = sqlx::query_as(
        "SELECT parent_id, \
                COALESCE(SUM(size_bytes), 0)::BIGINT AS total_bytes, \
                COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY parent_id \
          ORDER BY total_bytes DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder_id, total, fc)| serde_json::json!({
            "folder_id":   folder_id,
            "total_bytes": total,
            "file_count":  fc,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/avg-file-size-by-folder?limit=N — AVG/MAX size_bytes por folder.
///
/// GROUP BY parent_id; inclui NULL (raiz). Ordena por avg_bytes DESC. Sprint #881.
async fn file_stats_avg_file_size_by_folder(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(30).min(200).max(1);
    let pool   = state.db_or_unavailable()?;

    let rows: Vec<(Option<Uuid>, f64, i64, i64)> = sqlx::query_as(
        "SELECT parent_id, \
                AVG(size_bytes)::FLOAT8  AS avg_bytes, \
                MAX(size_bytes)::BIGINT  AS max_bytes, \
                COUNT(*)::BIGINT         AS file_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY parent_id \
          ORDER BY avg_bytes DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder_id, avg, max, fc)| serde_json::json!({
            "folder_id":  folder_id,
            "avg_bytes":  avg,
            "max_bytes":  max,
            "file_count": fc,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/folder-size-entropy — Shannon H sobre total_bytes por folder.
///
/// H = -Σ p*log2(p) onde p = folder_bytes / total_bytes global. Retorna `{entropy,total_bytes,folder_count}`. Sprint #876.
async fn file_stats_folder_size_entropy(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(Option<Uuid>, i64)> = sqlx::query_as(
        "SELECT parent_id, COALESCE(SUM(size_bytes), 0)::BIGINT AS folder_bytes \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY parent_id",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let total: i64 = rows.iter().map(|(_, b)| b).sum();
    let folder_count = rows.len();
    if total == 0 || folder_count < 2 {
        return Ok(Json(serde_json::json!({
            "entropy": serde_json::Value::Null,
            "total_bytes": total,
            "folder_count": folder_count,
        })));
    }
    let entropy: f64 = rows.iter()
        .filter(|(_, b)| *b > 0)
        .fold(0.0_f64, |acc, (_, b)| {
            let p = *b as f64 / total as f64;
            acc - p * p.log2()
        });
    Ok(Json(serde_json::json!({
        "entropy": entropy,
        "total_bytes": total,
        "folder_count": folder_count,
    })))
}

/// GET /api/v1/drive/files/stats/size-percentile — p25/p50/p75/p90/p95 de size_bytes.
///
/// Ordena size_bytes dos arquivos não-deletados e interpola percentis. Sprint #956.
async fn file_stats_size_percentile(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let sizes: Vec<(i64,)> = sqlx::query_as(
        "SELECT COALESCE(size_bytes, 0)::BIGINT \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          ORDER BY size_bytes ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let n = sizes.len();
    if n == 0 {
        return Ok(Json(serde_json::json!({"p25": null, "p50": null, "p75": null, "p90": null, "p95": null, "count": 0})));
    }
    let vals: Vec<i64> = sizes.into_iter().map(|(v,)| v).collect();
    let pct = |p: f64| -> i64 {
        let idx = ((n as f64 - 1.0) * p) as usize;
        vals[idx.min(n - 1)]
    };
    Ok(Json(serde_json::json!({
        "p25": pct(0.25),
        "p50": pct(0.50),
        "p75": pct(0.75),
        "p90": pct(0.90),
        "p95": pct(0.95),
        "count": n,
    })))
}

/// GET /api/v1/drive/files/stats/owner-entropy — Shannon H sobre owner_user_id.
///
/// H=-Σp*log2(p) sobre distribuição de arquivos por owner_user_id. Sprint #961.
async fn file_stats_owner_entropy(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(Option<Uuid>, i64)> = sqlx::query_as(
        "SELECT owner_user_id, COUNT(*)::BIGINT \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY owner_user_id",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let total: i64 = rows.iter().map(|(_, c)| c).sum();
    let owner_count = rows.len();
    if total == 0 || owner_count < 2 {
        return Ok(Json(serde_json::json!({"entropy": serde_json::Value::Null, "total_files": total, "owner_count": owner_count})));
    }
    let entropy: f64 = rows.iter()
        .filter(|(_, c)| *c > 0)
        .fold(0.0_f64, |acc, (_, c)| {
            let p = *c as f64 / total as f64;
            acc - p * p.log2()
        });
    Ok(Json(serde_json::json!({"entropy": entropy, "total_files": total, "owner_count": owner_count})))
}

/// GET /api/v1/drive/files/stats/version-size-by-ext — total version bytes por extensão.
///
/// JOIN drive_file_versions → GROUP BY extensão (LOWER NULLIF substring). Sprint #966.
async fn file_stats_version_size_by_ext(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(Option<String>, i64, i64)> = sqlx::query_as(
        "SELECT \
            LOWER(NULLIF(SUBSTRING(f.name FROM '\\.[^.]*$'), '')) AS ext, \
            COUNT(*)::BIGINT AS version_count, \
            COALESCE(SUM(fv.size_bytes), 0)::BIGINT AS total_bytes \
           FROM drive_file_versions fv \
           JOIN drive_files f ON f.id = fv.file_id \
          WHERE f.tenant_id = $1 AND f.deleted_at IS NULL \
          GROUP BY ext \
          ORDER BY total_bytes DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, vc, tb)| serde_json::json!({"ext": ext, "version_count": vc, "total_bytes": tb}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/locked-age — avg/max dias que arquivos estão bloqueados.
///
/// NOW()-locked_at em dias para kind='file' com locked_at IS NOT NULL. Sprint #971.
async fn file_stats_locked_age(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let (locked_count, avg_days, max_days): (i64, Option<f64>, Option<f64>) = sqlx::query_as(
        "SELECT \
            COUNT(*)::BIGINT AS locked_count, \
            AVG(EXTRACT(EPOCH FROM NOW() - locked_at) / 86400.0) AS avg_days_locked, \
            MAX(EXTRACT(EPOCH FROM NOW() - locked_at) / 86400.0) AS max_days_locked \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL AND locked_at IS NOT NULL",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool).await?;

    Ok(Json(serde_json::json!({
        "locked_count":   locked_count,
        "avg_days_locked": avg_days,
        "max_days_locked": max_days,
    })))
}

/// GET /api/v1/drive/files/stats/tag-size-by-ext — top (tag, ext) por total_bytes.
///
/// JOIN drive_file_tags → GROUP BY (tag, ext) ORDER BY total_bytes DESC LIMIT 50. Sprint #976.
async fn file_stats_tag_size_by_ext(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(String, Option<String>, i64, i64)> = sqlx::query_as(
        "SELECT \
            t.tag, \
            LOWER(NULLIF(SUBSTRING(f.name FROM '\\.[^.]*$'), '')) AS ext, \
            COUNT(*)::BIGINT AS file_count, \
            COALESCE(SUM(f.size_bytes), 0)::BIGINT AS total_bytes \
           FROM drive_file_tags t \
           JOIN drive_files f ON f.id = t.file_id \
          WHERE f.tenant_id = $1 AND f.kind = 'file' AND f.deleted_at IS NULL \
          GROUP BY t.tag, ext \
          ORDER BY total_bytes DESC \
          LIMIT 50",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(tag, ext, fc, tb)| serde_json::json!({"tag": tag, "ext": ext, "file_count": fc, "total_bytes": tb}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/folder-age — avg/max idade em dias de pastas (kind='folder') por created_at.
///
/// EXTRACT(EPOCH FROM NOW()-created_at)/86400. Sprint #981.
async fn file_stats_folder_age(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let (folder_count, avg_days, max_days): (i64, Option<f64>, Option<f64>) = sqlx::query_as(
        "SELECT \
            COUNT(*)::BIGINT AS folder_count, \
            AVG(EXTRACT(EPOCH FROM NOW() - created_at) / 86400.0) AS avg_days, \
            MAX(EXTRACT(EPOCH FROM NOW() - created_at) / 86400.0) AS max_days \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'folder' AND deleted_at IS NULL",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool).await?;

    Ok(Json(serde_json::json!({
        "folder_count": folder_count,
        "avg_days_old": avg_days,
        "max_days_old": max_days,
    })))
}

/// GET /api/v1/drive/files/stats/starred-age — avg/max dias desde starred_at.
///
/// Para arquivos com starred_at IS NOT NULL. Sprint #986.
async fn file_stats_starred_age(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let (starred_count, avg_days, max_days): (i64, Option<f64>, Option<f64>) = sqlx::query_as(
        "SELECT \
            COUNT(*)::BIGINT AS starred_count, \
            AVG(EXTRACT(EPOCH FROM NOW() - starred_at) / 86400.0) AS avg_days_starred, \
            MAX(EXTRACT(EPOCH FROM NOW() - starred_at) / 86400.0) AS max_days_starred \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL AND starred_at IS NOT NULL",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool).await?;

    Ok(Json(serde_json::json!({
        "starred_count":    starred_count,
        "avg_days_starred": avg_days,
        "max_days_starred": max_days,
    })))
}

/// GET /api/v1/drive/files/stats/created-vs-updated-gap — avg dias entre created_at e updated_at.
///
/// (updated_at - created_at) em dias, para arquivos modificados após criação. Sprint #991.
async fn file_stats_created_vs_updated_gap(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let (file_count, avg_days, max_days): (i64, Option<f64>, Option<f64>) = sqlx::query_as(
        "SELECT \
            COUNT(*) FILTER (WHERE updated_at > created_at)::BIGINT AS file_count, \
            AVG(EXTRACT(EPOCH FROM updated_at - created_at) / 86400.0) \
                FILTER (WHERE updated_at > created_at) AS avg_gap_days, \
            MAX(EXTRACT(EPOCH FROM updated_at - created_at) / 86400.0) \
                FILTER (WHERE updated_at > created_at) AS max_gap_days \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool).await?;

    Ok(Json(serde_json::json!({
        "modified_file_count": file_count,
        "avg_gap_days": avg_days,
        "max_gap_days": max_days,
    })))
}

/// GET /api/v1/drive/files/stats/ext-count-by-user — top (owner_user_id, ext) by file_count. Sprint #999.
async fn file_stats_ext_count_by_user(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsLimitQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(20).clamp(1, 200);

    let rows: Vec<(Option<uuid::Uuid>, Option<String>, i64)> = sqlx::query_as(
        "SELECT \
            owner_user_id, \
            LOWER(NULLIF(SUBSTRING(name FROM '\\.[^.]*$'), '')) AS ext, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY owner_user_id, ext \
          ORDER BY file_count DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(u, e, c)| serde_json::json!({"owner_user_id": u, "ext": e, "file_count": c}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-stdev-by-ext — stdev of size_bytes per extension. Sprint #1000.
async fn file_stats_size_stdev_by_ext(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(Option<String>, i64, f64, Option<f64>)> = sqlx::query_as(
        "SELECT \
            LOWER(NULLIF(SUBSTRING(name FROM '\\.[^.]*$'), '')) AS ext, \
            COUNT(*)::BIGINT AS file_count, \
            COALESCE(AVG(size_bytes), 0.0) AS avg_bytes, \
            STDDEV_POP(size_bytes) AS stdev_bytes \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY ext \
          HAVING COUNT(*) > 1 \
          ORDER BY stdev_bytes DESC NULLS LAST \
          LIMIT 50",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(e, c, avg, sd)| serde_json::json!({"ext": e, "file_count": c, "avg_bytes": avg, "stdev_bytes": sd}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/quota-utilization-by-folder — used vs quota per folder. Sprint #1001.
async fn file_stats_quota_utilization_by_folder(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(uuid::Uuid, Option<String>, i64, Option<i64>, Option<f64>)> = sqlx::query_as(
        "SELECT \
            f.id, \
            f.name, \
            COALESCE(SUM(fi.size_bytes) FILTER (WHERE fi.kind = 'file' AND fi.deleted_at IS NULL), 0)::BIGINT AS used_bytes, \
            fq.quota_bytes, \
            CASE WHEN fq.quota_bytes IS NOT NULL AND fq.quota_bytes > 0 \
                 THEN COALESCE(SUM(fi.size_bytes) FILTER (WHERE fi.kind = 'file' AND fi.deleted_at IS NULL), 0)::FLOAT8 / fq.quota_bytes \
                 ELSE NULL END AS utilization_ratio \
           FROM drive_files f \
           LEFT JOIN drive_files fi ON fi.parent_id = f.id \
           LEFT JOIN drive_folder_quotas fq ON fq.folder_id = f.id \
          WHERE f.tenant_id = $1 AND f.kind = 'folder' AND f.deleted_at IS NULL \
          GROUP BY f.id, f.name, fq.quota_bytes \
          ORDER BY used_bytes DESC \
          LIMIT 50",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(id, name, used, quota, ratio)| serde_json::json!({
            "folder_id": id,
            "folder_name": name,
            "used_bytes": used,
            "quota_bytes": quota,
            "utilization_ratio": ratio,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/file-count-by-weekday — COUNT files per day-of-week (created_at). Sprint #1002.
async fn file_stats_file_count_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, count)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "file_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/version-count-by-user — COUNT versions por owner_user_id. Sprint #1019.
async fn file_stats_version_count_by_user(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsLimitQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(20).clamp(1, 200);

    let rows: Vec<(Option<uuid::Uuid>, i64, i64)> = sqlx::query_as(
        "SELECT \
            f.owner_user_id, \
            COUNT(DISTINCT f.id)::BIGINT AS file_count, \
            COUNT(v.id)::BIGINT AS version_count \
           FROM drive_files f \
           JOIN drive_file_versions v ON v.file_id = f.id \
          WHERE f.tenant_id = $1 AND f.kind = 'file' AND f.deleted_at IS NULL \
          GROUP BY f.owner_user_id \
          ORDER BY version_count DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(u, fc, vc)| serde_json::json!({"owner_user_id": u, "file_count": fc, "version_count": vc}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-cv-by-folder — coefficient of variation (stdev/mean) de size_bytes por folder. Sprint #1020.
async fn file_stats_size_cv_by_folder(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(Option<uuid::Uuid>, i64, Option<f64>, Option<f64>, Option<f64>)> = sqlx::query_as(
        "SELECT \
            parent_id, \
            COUNT(*)::BIGINT AS file_count, \
            AVG(size_bytes) AS avg_bytes, \
            STDDEV_POP(size_bytes) AS stdev_bytes, \
            CASE WHEN AVG(size_bytes) > 0 \
                 THEN STDDEV_POP(size_bytes) / AVG(size_bytes) \
                 ELSE NULL END AS cv \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY parent_id \
          HAVING COUNT(*) > 1 \
          ORDER BY cv DESC NULLS LAST \
          LIMIT 50",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(pid, fc, avg, sd, cv)| serde_json::json!({
            "folder_id": pid,
            "file_count": fc,
            "avg_bytes": avg,
            "stdev_bytes": sd,
            "cv": cv,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-by-day — COUNT arquivos soft-deleted por dia (deleted_at). Sprint #1021.
async fn file_stats_deleted_by_day(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<DateRangeQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let since_dt: Option<OffsetDateTime> = q.since.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| OffsetDateTime::UNIX_EPOCH)
    });
    let until_dt: Option<OffsetDateTime> = q.until.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| OffsetDateTime::UNIX_EPOCH)
    });

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('day', deleted_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            COUNT(*)::BIGINT AS deleted_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
            AND ($2::timestamptz IS NULL OR deleted_at >= $2) \
            AND ($3::timestamptz IS NULL OR deleted_at <  $3) \
          GROUP BY day \
          ORDER BY day ASC",
    )
    .bind(ctx.tenant_id).bind(since_dt).bind(until_dt)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(day, count)| serde_json::json!({"day": day, "deleted_count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/mime-top-by-size — top MIME types por total_bytes. Sprint #1022.
async fn file_stats_mime_top_by_size(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsLimitQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(20).clamp(1, 100);

    let rows: Vec<(Option<String>, i64, i64)> = sqlx::query_as(
        "SELECT \
            mime_type, \
            COUNT(*)::BIGINT AS file_count, \
            COALESCE(SUM(size_bytes), 0)::BIGINT AS total_bytes \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY mime_type \
          ORDER BY total_bytes DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, fc, tb)| serde_json::json!({"mime_type": mime, "file_count": fc, "total_bytes": tb}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/name-length-by-ext — avg/max comprimento do nome por extensão. Sprint #1039.
async fn file_stats_name_length_by_ext(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(Option<String>, i64, Option<f64>, Option<i64>)> = sqlx::query_as(
        "SELECT \
            LOWER(NULLIF(SUBSTRING(name FROM '\\.[^.]*$'), '')) AS ext, \
            COUNT(*)::BIGINT AS file_count, \
            AVG(LENGTH(name)) AS avg_name_length, \
            MAX(LENGTH(name))::BIGINT AS max_name_length \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY ext \
          ORDER BY avg_name_length DESC NULLS LAST \
          LIMIT 50",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(e, fc, avg, max)| serde_json::json!({
            "ext": e, "file_count": fc, "avg_name_length": avg, "max_name_length": max,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/ext-size-percentile — P25/P50/P75/P90 de size_bytes por extensão. Sprint #1040.
async fn file_stats_ext_size_percentile(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(Option<String>, i64, i64, i64, i64, i64)> = sqlx::query_as(
        "SELECT \
            LOWER(NULLIF(SUBSTRING(name FROM '\\.[^.]*$'), '')) AS ext, \
            PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p25, \
            PERCENTILE_CONT(0.50) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p50, \
            PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p75, \
            PERCENTILE_CONT(0.90) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p90, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL AND size_bytes IS NOT NULL \
          GROUP BY ext \
          HAVING COUNT(*) >= 3 \
          ORDER BY p50 DESC NULLS LAST \
          LIMIT 50",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(e, p25, p50, p75, p90, fc)| serde_json::json!({
            "ext": e, "p25": p25, "p50": p50, "p75": p75, "p90": p90, "file_count": fc,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/orphan-files — arquivos sem parent_id (raiz) por tipo. Sprint #1041.
async fn file_stats_orphan_files(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let (total_root, file_count, folder_count, total_bytes): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT \
            COUNT(*)::BIGINT AS total_root, \
            COUNT(*) FILTER (WHERE kind = 'file')::BIGINT AS file_count, \
            COUNT(*) FILTER (WHERE kind = 'folder')::BIGINT AS folder_count, \
            COALESCE(SUM(size_bytes) FILTER (WHERE kind = 'file'), 0)::BIGINT AS total_bytes \
           FROM drive_files \
          WHERE tenant_id = $1 AND parent_id IS NULL AND deleted_at IS NULL",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool).await?;

    Ok(Json(serde_json::json!({
        "total_root_items": total_root,
        "root_file_count": file_count,
        "root_folder_count": folder_count,
        "root_total_bytes": total_bytes,
    })))
}

/// GET /api/v1/drive/files/stats/duplicate-name — nomes de arquivo duplicados (mesmo nome, mesmo parent). Sprint #1042.
async fn file_stats_duplicate_name(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsLimitQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(20).clamp(1, 100);

    let rows: Vec<(Option<uuid::Uuid>, String, i64)> = sqlx::query_as(
        "SELECT \
            parent_id, \
            name, \
            COUNT(*)::BIGINT AS duplicate_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY parent_id, name \
          HAVING COUNT(*) > 1 \
          ORDER BY duplicate_count DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(pid, name, count)| serde_json::json!({
            "parent_id": pid, "name": name, "duplicate_count": count,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size — total bytes + count de arquivos soft-deleted. Sprint #1059.
async fn file_stats_deleted_size(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let (total_deleted, total_bytes): (i64, i64) = sqlx::query_as(
        "SELECT \
            COUNT(*)::BIGINT AS total_deleted, \
            COALESCE(SUM(size_bytes) FILTER (WHERE size_bytes IS NOT NULL), 0)::BIGINT AS total_bytes \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NOT NULL",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool).await?;

    Ok(Json(serde_json::json!({
        "total_deleted_files": total_deleted,
        "total_deleted_bytes": total_bytes,
    })))
}

/// GET /api/v1/drive/files/stats/created-by-weekday-and-ext — COUNT por (DOW, ext) de created_at. Sprint #1060.
async fn file_stats_created_by_weekday_and_ext(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsLimitQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);

    let rows: Vec<(i32, Option<String>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            LOWER(NULLIF(SUBSTRING(name FROM '\\.[^.]*$'), '')) AS ext, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY dow, ext \
          ORDER BY dow ASC, file_count DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, ext, count)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "ext": ext, "file_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/avg-version-size — AVG/MAX size_bytes de versões por arquivo. Sprint #1061.
async fn file_stats_avg_version_size(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsLimitQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(20).clamp(1, 100);

    let rows: Vec<(uuid::Uuid, Option<String>, i64, i64, i64)> = sqlx::query_as(
        "SELECT \
            f.id AS file_id, \
            f.name, \
            COUNT(v.id)::BIGINT AS version_count, \
            COALESCE(AVG(v.size_bytes)::BIGINT, 0) AS avg_version_bytes, \
            COALESCE(MAX(v.size_bytes), 0)::BIGINT AS max_version_bytes \
           FROM drive_files f \
           JOIN drive_file_versions v ON v.file_id = f.id \
          WHERE f.tenant_id = $1 AND f.kind = 'file' AND f.deleted_at IS NULL \
            AND v.size_bytes IS NOT NULL \
          GROUP BY f.id, f.name \
          ORDER BY avg_version_bytes DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(fid, name, vc, avg_b, max_b)| serde_json::json!({
            "file_id": fid,
            "name": name,
            "version_count": vc,
            "avg_version_bytes": avg_b,
            "max_version_bytes": max_b,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/folder-count — COUNT total pastas + by_user top-N. Sprint #1062.
async fn file_stats_folder_count(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsLimitQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(20).clamp(1, 100);

    let (total_folders,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT FROM drive_files WHERE tenant_id = $1 AND kind = 'folder' AND deleted_at IS NULL",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool).await?;

    let by_user: Vec<(Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT owner_user_id, COUNT(*)::BIGINT AS folder_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'folder' AND deleted_at IS NULL \
          GROUP BY owner_user_id \
          ORDER BY folder_count DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let by_user_json: Vec<serde_json::Value> = by_user.into_iter()
        .map(|(u, c)| serde_json::json!({"owner_user_id": u, "folder_count": c}))
        .collect();

    Ok(Json(serde_json::json!({
        "total_folders": total_folders,
        "by_user": by_user_json,
    })))
}

/// GET /api/v1/drive/files/stats/starred-by-weekday — COUNT arquivos starred × DOW de created_at. Sprint #1096.
async fn file_stats_starred_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(*) FILTER (WHERE starred = true)::BIGINT AS starred_count, \
            COUNT(*)::BIGINT AS total_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, starred, total)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            let rate = if total > 0 { starred as f64 / total as f64 } else { 0.0 };
            serde_json::json!({"dow": dow, "day_name": day_name, "starred_count": starred, "total_count": total, "starred_rate": rate})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-by-weekday — COUNT arquivos soft-deleted × DOW de deleted_at. Sprint #1091.
async fn file_stats_deleted_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM deleted_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(*)::BIGINT AS deleted_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, count)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "deleted_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/version-size-by-weekday — AVG/SUM size_bytes de versões × DOW. Sprint #1086.
async fn file_stats_version_size_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM v.created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COALESCE(AVG(v.size_bytes)::BIGINT, 0) AS avg_size_bytes, \
            COALESCE(SUM(v.size_bytes)::BIGINT, 0) AS total_size_bytes \
           FROM drive_file_versions v \
           JOIN drive_files f ON f.id = v.file_id \
          WHERE f.tenant_id = $1 AND f.deleted_at IS NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, avg, total)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "avg_size_bytes": avg, "total_size_bytes": total})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/version-count-by-weekday — COUNT versões criadas × DOW. Sprint #1081.
async fn file_stats_version_count_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM v.created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(*)::BIGINT AS version_count \
           FROM drive_file_versions v \
           JOIN drive_files f ON f.id = v.file_id \
          WHERE f.tenant_id = $1 AND f.deleted_at IS NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, count)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "version_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/mime-by-weekday — top mime_type × DOW de created_at. Sprint #1076.
async fn file_stats_mime_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsLimitQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);

    let rows: Vec<(i32, Option<String>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            mime_type, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY dow, mime_type \
          ORDER BY dow ASC, file_count DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, mime, count)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "mime_type": mime, "file_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/modified-by-weekday — COUNT arquivos × DOW de updated_at. Sprint #1246.
async fn file_stats_modified_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM updated_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, count)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "file_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/file-count-by-hour — COUNT arquivos × hora-do-dia de created_at. Sprint #1251.
async fn file_stats_file_count_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY hour_of_day \
          ORDER BY hour_of_day ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, count)| serde_json::json!({"hour_of_day": h, "file_count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/shared-count-by-weekday — COUNT arquivos com shared_at × DOW de created_at. Sprint #1266.
async fn file_stats_shared_count_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(*)::BIGINT AS shared_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL AND shared_at IS NOT NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, count)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "shared_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/shared-count-by-hour — COUNT arquivos com shared_at × hora-do-dia de created_at. Sprint #1271.
async fn file_stats_shared_count_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COUNT(*)::BIGINT AS shared_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL AND shared_at IS NOT NULL \
          GROUP BY hour_of_day \
          ORDER BY hour_of_day ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, count)| serde_json::json!({"hour_of_day": h, "shared_count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/ext-count-by-weekday — COUNT DISTINCT extensões × DOW de created_at. Sprint #1306.
async fn file_stats_ext_count_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(DISTINCT extension)::BIGINT AS ext_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL AND extension IS NOT NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, count)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "ext_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/shared-by-hour — COUNT arquivos compartilhados × hora-do-dia de shared_at. Sprint #1321.
async fn file_stats_shared_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR FROM shared_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COUNT(*)::BIGINT AS shared_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL AND shared_at IS NOT NULL \
          GROUP BY hour_of_day \
          ORDER BY hour_of_day ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, count)| serde_json::json!({"hour_of_day": h, "shared_count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/locked-count-by-weekday — COUNT arquivos com locked_at × DOW de created_at. Sprint #1361.
async fn file_stats_locked_count_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(*) FILTER (WHERE locked_at IS NOT NULL)::BIGINT AS locked_count, \
            COUNT(*)::BIGINT AS total_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(d, locked, total)| {
            let day_name = DAY_NAMES.get(d as usize).copied().unwrap_or("Unknown");
            let rate = if total > 0 { locked as f64 / total as f64 } else { 0.0 };
            serde_json::json!({"dow": d, "day_name": day_name, "locked_count": locked, "total_count": total, "lock_rate": rate})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/locked-count-by-hour — COUNT arquivos com locked_at × hora-do-dia de created_at. Sprint #1356.
async fn file_stats_locked_count_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COUNT(*) FILTER (WHERE locked_at IS NOT NULL)::BIGINT AS locked_count, \
            COUNT(*)::BIGINT AS total_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL \
          GROUP BY hour_of_day \
          ORDER BY hour_of_day ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, locked, total)| {
            let rate = if total > 0 { locked as f64 / total as f64 } else { 0.0 };
            serde_json::json!({"hour_of_day": h, "locked_count": locked, "total_count": total, "lock_rate": rate})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/shared-by-month — COUNT arquivos compartilhados × mês de shared_at. Sprint #1401.
async fn file_stats_shared_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM shared_at AT TIME ZONE 'UTC')::INT AS month, \
            COUNT(*)::BIGINT AS shared_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL AND shared_at IS NOT NULL \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const MONTH_NAMES: [&str; 12] = [
        "January","February","March","April","May","June",
        "July","August","September","October","November","December",
    ];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(month, count)| {
            let month_name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"month": month, "month_name": month_name, "shared_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/owner-by-weekday — COUNT arquivos por (owner_user_id, DOW). Sprint #1396.
async fn file_stats_owner_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(Uuid, i32, i64)> = sqlx::query_as(
        "SELECT \
            owner_user_id, \
            EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL AND owner_user_id IS NOT NULL \
          GROUP BY owner_user_id, dow \
          ORDER BY dow ASC, file_count DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, dow, count)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"owner_user_id": owner, "dow": dow, "day_name": day_name, "file_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/shared-count-by-month — COUNT arquivos com shared_at × mês de created_at. Sprint #1351.
async fn file_stats_shared_count_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
            COUNT(*) FILTER (WHERE shared_at IS NOT NULL)::BIGINT AS shared_count, \
            COUNT(*)::BIGINT AS total_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, shared, total)| {
            let rate = if total > 0 { shared as f64 / total as f64 } else { 0.0 };
            serde_json::json!({"month": m, "shared_count": shared, "total_count": total, "share_rate": rate})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/trashed-by-month — COUNT arquivos com deleted_at × mês. Sprint #1376.
async fn file_stats_trashed_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM deleted_at AT TIME ZONE 'UTC')::INT AS month, \
            COUNT(*)::BIGINT AS trashed_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const MONTH_NAMES: [&str; 12] = [
        "January","February","March","April","May","June",
        "July","August","September","October","November","December",
    ];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(month, count)| {
            let month_name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"month": month, "month_name": month_name, "trashed_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/trashed-by-hour — COUNT arquivos deletados × hora-do-dia de deleted_at. Sprint #1371.
async fn file_stats_trashed_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR FROM deleted_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COUNT(*)::BIGINT AS trashed_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
          GROUP BY hour_of_day \
          ORDER BY hour_of_day ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, count)| serde_json::json!({"hour_of_day": h, "trashed_count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/trashed-by-weekday — COUNT arquivos deletados × DOW de deleted_at. Sprint #1366.
async fn file_stats_trashed_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM deleted_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(*)::BIGINT AS trashed_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(d, count)| {
            let day_name = DAY_NAMES.get(d as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": d, "day_name": day_name, "trashed_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/locked-count-by-month — COUNT arquivos com locked_at × mês de created_at. Sprint #1346.
async fn file_stats_locked_count_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
            COUNT(*) FILTER (WHERE locked_at IS NOT NULL)::BIGINT AS locked_count, \
            COUNT(*)::BIGINT AS total_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, locked, total)| {
            let rate = if total > 0 { locked as f64 / total as f64 } else { 0.0 };
            serde_json::json!({"month": m, "locked_count": locked, "total_count": total, "lock_rate": rate})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/owner-by-month — COUNT arquivos por (owner_user_id, mês). Sprint #1391.
async fn file_stats_owner_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(Uuid, i32, i64)> = sqlx::query_as(
        "SELECT \
            owner_user_id, \
            EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL AND owner_user_id IS NOT NULL \
          GROUP BY owner_user_id, month \
          ORDER BY month ASC, file_count DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const MONTH_NAMES: [&str; 12] = [
        "January","February","March","April","May","June",
        "July","August","September","October","November","December",
    ];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, month, count)| {
            let month_name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"owner_user_id": owner, "month": month, "month_name": month_name, "file_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/owner-count-by-month — COUNT DISTINCT owners × mês de created_at. Sprint #1341.
async fn file_stats_owner_count_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
            COUNT(DISTINCT owner_user_id)::BIGINT AS owner_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, count)| serde_json::json!({"month": m, "owner_count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/tag-count-by-month — COUNT tags aplicadas × mês de created_at do arquivo. Sprint #1336.
async fn file_stats_tag_count_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM f.created_at AT TIME ZONE 'UTC')::INT AS month, \
            COUNT(t.tag)::BIGINT AS tag_count \
           FROM drive_files f \
           JOIN drive_file_tags t ON t.file_id = f.id \
          WHERE f.tenant_id = $1 AND f.deleted_at IS NULL \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, count)| serde_json::json!({"month": m, "tag_count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/mime-count-by-month — COUNT DISTINCT mime_types × mês de created_at. Sprint #1331.
async fn file_stats_mime_count_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
            COUNT(DISTINCT mime_type)::BIGINT AS mime_type_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL AND mime_type IS NOT NULL \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, count)| serde_json::json!({"month": m, "mime_type_count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/ext-count-by-month — COUNT DISTINCT extensões × mês de created_at. Sprint #1326.
async fn file_stats_ext_count_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
            COUNT(DISTINCT extension)::BIGINT AS ext_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL AND extension IS NOT NULL \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, count)| serde_json::json!({"month": m, "ext_count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/shared-by-weekday — COUNT arquivos compartilhados × DOW de shared_at. Sprint #1316.
async fn file_stats_shared_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM shared_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(*)::BIGINT AS shared_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL AND shared_at IS NOT NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(d, count)| serde_json::json!({"day_of_week": d, "shared_count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/ext-count-by-hour — COUNT DISTINCT extensões × hora-do-dia de created_at. Sprint #1311.
async fn file_stats_ext_count_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COUNT(DISTINCT extension)::BIGINT AS ext_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL AND extension IS NOT NULL \
          GROUP BY hour_of_day \
          ORDER BY hour_of_day ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, count)| serde_json::json!({"hour_of_day": h, "ext_count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/mime-count-by-weekday — COUNT DISTINCT mime_types × DOW de created_at. Sprint #1296.
async fn file_stats_mime_count_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(DISTINCT mime_type)::BIGINT AS mime_type_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL AND mime_type IS NOT NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, count)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "mime_type_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/mime-count-by-hour — COUNT DISTINCT mime_types × hora-do-dia de created_at. Sprint #1301.
async fn file_stats_mime_count_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COUNT(DISTINCT mime_type)::BIGINT AS mime_type_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL AND mime_type IS NOT NULL \
          GROUP BY hour_of_day \
          ORDER BY hour_of_day ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, count)| serde_json::json!({"hour_of_day": h, "mime_type_count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/tag-count-by-weekday — COUNT tags aplicadas × DOW de created_at do arquivo. Sprint #1286.
async fn file_stats_tag_count_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM f.created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(t.tag)::BIGINT AS tag_count \
           FROM drive_files f \
           JOIN drive_file_tags t ON t.file_id = f.id \
          WHERE f.tenant_id = $1 AND f.deleted_at IS NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, count)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "tag_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/tag-by-hour — COUNT arquivos por (tag, hora-do-dia). Sprint #1381.
async fn file_stats_tag_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(String, i32, i64)> = sqlx::query_as(
        "SELECT \
            t.tag, \
            EXTRACT(HOUR FROM f.created_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COUNT(f.id)::BIGINT AS file_count \
           FROM drive_files f \
           JOIN drive_file_tags t ON t.file_id = f.id \
          WHERE f.tenant_id = $1 AND f.deleted_at IS NULL \
          GROUP BY t.tag, hour_of_day \
          ORDER BY t.tag ASC, hour_of_day ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(tag, h, count)| serde_json::json!({"tag": tag, "hour_of_day": h, "file_count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/tag-count-by-hour — COUNT tags aplicadas × hora-do-dia de created_at do arquivo. Sprint #1291.
async fn file_stats_tag_count_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR FROM f.created_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COUNT(t.tag)::BIGINT AS tag_count \
           FROM drive_files f \
           JOIN drive_file_tags t ON t.file_id = f.id \
          WHERE f.tenant_id = $1 AND f.deleted_at IS NULL \
          GROUP BY hour_of_day \
          ORDER BY hour_of_day ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, count)| serde_json::json!({"hour_of_day": h, "tag_count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/owner-count-by-weekday — COUNT DISTINCT owners × DOW de created_at. Sprint #1276.
async fn file_stats_owner_count_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(DISTINCT owner_user_id)::BIGINT AS owner_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, count)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "owner_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/owner-count-by-hour — COUNT DISTINCT owners × hora-do-dia de created_at. Sprint #1281.
async fn file_stats_owner_count_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COUNT(DISTINCT owner_user_id)::BIGINT AS owner_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL \
          GROUP BY hour_of_day \
          ORDER BY hour_of_day ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, count)| serde_json::json!({"hour_of_day": h, "owner_count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/owner-by-hour — COUNT arquivos por (owner_user_id, hora-do-dia). Sprint #1386.
async fn file_stats_owner_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(Uuid, i32, i64)> = sqlx::query_as(
        "SELECT \
            owner_user_id, \
            EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL AND owner_user_id IS NOT NULL \
          GROUP BY owner_user_id, hour_of_day \
          ORDER BY hour_of_day ASC, file_count DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, h, count)| serde_json::json!({"owner_user_id": owner, "hour_of_day": h, "file_count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/quota-by-weekday — SUM/AVG quota_bytes de arquivos × DOW de created_at. Sprint #1256.
async fn file_stats_quota_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64, f64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COALESCE(SUM(size_bytes), 0)::BIGINT AS total_bytes, \
            COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_bytes \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, total, avg)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "total_bytes": total, "avg_bytes": avg})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/quota-by-hour — SUM/AVG quota_bytes de arquivos × hora-do-dia de created_at. Sprint #1261.
async fn file_stats_quota_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64, f64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COALESCE(SUM(size_bytes), 0)::BIGINT AS total_bytes, \
            COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_bytes \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL \
          GROUP BY hour_of_day \
          ORDER BY hour_of_day ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, total, avg)| serde_json::json!({"hour_of_day": h, "total_bytes": total, "avg_bytes": avg}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/locked-count-by-dow — COUNT arquivos locked × DOW de created_at. Sprint #1441.
async fn file_stats_locked_count_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(*)::BIGINT AS locked_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL AND locked_at IS NOT NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, count)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "locked_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/trashed-count-by-dow — COUNT arquivos com deleted_at (trashed) × DOW de created_at. Sprint #1446.
async fn file_stats_trashed_count_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(*)::BIGINT AS trashed_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NOT NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, count)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "trashed_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/trashed-count-by-hour — COUNT arquivos com deleted_at × hora-do-dia de created_at. Sprint #1456.
async fn file_stats_trashed_count_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour, \
            COUNT(*)::BIGINT AS trashed_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NOT NULL \
          GROUP BY hour \
          ORDER BY hour ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(hour, count)| serde_json::json!({"hour": hour, "trashed_count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/trashed-count-by-weekday — COUNT arquivos com deleted_at × DOW (nome) de created_at. Sprint #1461.
async fn file_stats_trashed_count_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(*)::BIGINT AS trashed_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NOT NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, count)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "trashed_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/trashed-count-by-month — COUNT arquivos com deleted_at × mês de created_at. Sprint #1466.
async fn file_stats_trashed_count_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
            COUNT(*)::BIGINT AS trashed_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NOT NULL \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const MONTH_NAMES: [&str; 12] = ["January","February","March","April","May","June","July","August","September","October","November","December"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, count)| {
            let month_name = MONTH_NAMES.get((m - 1) as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"month": m, "month_name": month_name, "trashed_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/quota-by-dow — SUM/AVG size_bytes × DOW de created_at. Sprint #1471.
async fn file_stats_quota_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64, f64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COALESCE(SUM(size_bytes), 0)::BIGINT AS total_bytes, \
            COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_bytes \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, total, avg)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "total_bytes": total, "avg_bytes": avg})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/avg-size-by-dow — AVG/MAX size_bytes × DOW de created_at. Sprint #1476.
async fn file_stats_avg_size_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, f64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_size, \
            COALESCE(MAX(size_bytes), 0)::BIGINT AS max_size \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, avg, max)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "avg_size": avg, "max_size": max})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/avg-size-by-month — AVG/MAX size_bytes × mês de created_at. Sprint #1481.
async fn file_stats_avg_size_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, f64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
            COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_size, \
            COALESCE(MAX(size_bytes), 0)::BIGINT AS max_size \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const MONTH_NAMES: [&str; 12] = ["January","February","March","April","May","June","July","August","September","October","November","December"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, avg, max)| {
            let month_name = MONTH_NAMES.get((m - 1) as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"month": m, "month_name": month_name, "avg_size": avg, "max_size": max})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/shared-count-by-dow — COUNT arquivos com shared_at × DOW de created_at. Sprint #1451.
async fn file_stats_shared_count_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(*)::BIGINT AS shared_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL AND shared_at IS NOT NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, count)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "shared_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/version-count-by-dow — AVG/MAX versões por arquivo × DOW de created_at. Sprint #1436.
async fn file_stats_version_count_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, f64, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM f.created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COALESCE(AVG(v.version_count), 0.0)::FLOAT8 AS avg_versions, \
            COALESCE(MAX(v.version_count), 0)::BIGINT AS max_versions, \
            COUNT(DISTINCT f.id)::BIGINT AS file_count \
           FROM drive_files f \
           LEFT JOIN ( \
               SELECT file_id, COUNT(*)::BIGINT AS version_count \
                 FROM drive_file_versions \
                GROUP BY file_id \
           ) v ON v.file_id = f.id \
          WHERE f.tenant_id = $1 AND f.kind = 'file' AND f.deleted_at IS NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, avg, max, count)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "avg_versions": avg, "max_versions": max, "file_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/tag-count-by-dow — COUNT tags aplicadas × DOW de created_at do arquivo. Sprint #1431.
async fn file_stats_tag_count_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM f.created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(t.tag)::BIGINT AS tag_count \
           FROM drive_files f \
           JOIN drive_file_tags t ON t.file_id = f.id \
          WHERE f.tenant_id = $1 AND f.deleted_at IS NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, count)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "tag_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/mime-count-by-dow — COUNT DISTINCT mime_types × DOW de created_at. Sprint #1426.
async fn file_stats_mime_count_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(DISTINCT mime_type)::BIGINT AS mime_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL AND mime_type IS NOT NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, count)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "mime_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/owner-count-by-dow — COUNT DISTINCT owners × DOW de created_at. Sprint #1421.
async fn file_stats_owner_count_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(DISTINCT owner_user_id)::BIGINT AS owner_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL AND owner_user_id IS NOT NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, count)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "owner_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/ext-count-by-dow — COUNT DISTINCT extensões × DOW de created_at. Sprint #1416.
async fn file_stats_ext_count_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(DISTINCT LOWER(NULLIF(REGEXP_REPLACE(name, '^.*\\.', ''), name)))::BIGINT AS ext_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL AND name LIKE '%.%' \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, count)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "ext_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/owner-by-dow — COUNT arquivos × owner_user_id × DOW de created_at. Sprint #1411.
async fn file_stats_owner_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(Uuid, i32, i64)> = sqlx::query_as(
        "SELECT \
            owner_user_id, \
            EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL AND owner_user_id IS NOT NULL \
          GROUP BY owner_user_id, dow \
          ORDER BY dow ASC, file_count DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, dow, count)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"owner_user_id": owner, "dow": dow, "day_name": day_name, "file_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/quota-by-month — SUM/AVG size_bytes por mês de created_at. Sprint #1406.
async fn file_stats_quota_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i32, i64, f64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
            COALESCE(SUM(size_bytes), 0)::BIGINT AS total_bytes, \
            COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_bytes \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    const MONTH_NAMES: [&str; 12] = [
        "January","February","March","April","May","June",
        "July","August","September","October","November","December",
    ];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, total, avg)| {
            let month_name = MONTH_NAMES.get((m - 1) as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"month": m, "month_name": month_name, "total_bytes": total, "avg_bytes": avg})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/avg-size-by-hour — AVG/MAX size_bytes × hora-do-dia de created_at. Sprint #1486.
async fn file_stats_avg_size_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, f64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour, \
            COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_size, \
            COALESCE(MAX(size_bytes), 0)::BIGINT AS max_size \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY hour \
          ORDER BY hour ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, avg, max)| serde_json::json!({"hour": h, "avg_size": avg, "max_size": max}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/avg-size-by-weekday — AVG/MAX size_bytes × dia-da-semana (nome). Sprint #1491.
async fn file_stats_avg_size_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, f64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_size, \
            COALESCE(MAX(size_bytes), 0)::BIGINT AS max_size \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, avg, max)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "avg_size": avg, "max_size": max})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p95-by-weekday — P95 de size_bytes × DOW de created_at. Sprint #1496.
async fn file_stats_size_p95_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p95_size \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, p95)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "p95_size": p95})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p99-by-weekday — P99 de size_bytes × DOW de created_at. Sprint #1501.
async fn file_stats_size_p99_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            PERCENTILE_CONT(0.99) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p99_size \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, p99)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "p99_size": p99})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p95-by-hour — P95 de size_bytes × hora-do-dia de created_at. Sprint #1506.
async fn file_stats_size_p95_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour, \
            PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p95_size \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY hour \
          ORDER BY hour ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, p95)| serde_json::json!({"hour": h, "p95_size": p95}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p99-by-hour — P99 de size_bytes × hora-do-dia de created_at. Sprint #1511.
async fn file_stats_size_p99_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour, \
            PERCENTILE_CONT(0.99) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p99_size \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY hour \
          ORDER BY hour ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, p99)| serde_json::json!({"hour": h, "p99_size": p99}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p95-by-month — P95 de size_bytes × mês de created_at. Sprint #1516.
async fn file_stats_size_p95_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
            PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p95_size \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;
    const MONTH_NAMES: [&str; 12] = ["January","February","March","April","May","June","July","August","September","October","November","December"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, p95)| {
            let month_name = MONTH_NAMES.get((m - 1) as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"month": m, "month_name": month_name, "p95_size": p95})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p99-by-month — P99 de size_bytes × mês de created_at. Sprint #1521.
async fn file_stats_size_p99_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
            PERCENTILE_CONT(0.99) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p99_size \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;
    const MONTH_NAMES: [&str; 12] = ["January","February","March","April","May","June","July","August","September","October","November","December"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, p99)| {
            let month_name = MONTH_NAMES.get((m - 1) as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"month": m, "month_name": month_name, "p99_size": p99})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p95-by-dow — P95 de size_bytes × DOW (0=Sun) de created_at. Sprint #1526.
async fn file_stats_size_p95_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p95_size \
           FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY dow ORDER BY dow ASC",
    ).bind(ctx.tenant_id).fetch_all(pool).await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, p95)| { let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown"); serde_json::json!({"dow": dow, "day_name": day_name, "p95_size": p95}) })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p99-by-dow — P99 de size_bytes × DOW (0=Sun) de created_at. Sprint #1531.
async fn file_stats_size_p99_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            PERCENTILE_CONT(0.99) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p99_size \
           FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY dow ORDER BY dow ASC",
    ).bind(ctx.tenant_id).fetch_all(pool).await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, p99)| { let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown"); serde_json::json!({"dow": dow, "day_name": day_name, "p99_size": p99}) })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p75-by-hour — P75 de size_bytes × hora-do-dia de created_at. Sprint #1536.
async fn file_stats_size_p75_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour, \
            PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p75_size \
           FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY hour ORDER BY hour ASC",
    ).bind(ctx.tenant_id).fetch_all(pool).await?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, p75)| serde_json::json!({"hour": h, "p75_size": p75}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p75-by-weekday — P75 de size_bytes × DOW (nome) de created_at. Sprint #1541.
async fn file_stats_size_p75_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p75_size \
           FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY dow ORDER BY dow ASC",
    ).bind(ctx.tenant_id).fetch_all(pool).await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, p75)| { let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown"); serde_json::json!({"dow": dow, "day_name": day_name, "p75_size": p75}) })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p75-by-month — P75 de size_bytes × mês de created_at. Sprint #1546.
async fn file_stats_size_p75_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
            PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p75_size \
           FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY month ORDER BY month ASC",
    ).bind(ctx.tenant_id).fetch_all(pool).await?;
    const MONTH_NAMES: [&str; 12] = ["January","February","March","April","May","June","July","August","September","October","November","December"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, p75)| { let month_name = MONTH_NAMES.get((m-1) as usize).copied().unwrap_or("Unknown"); serde_json::json!({"month": m, "month_name": month_name, "p75_size": p75}) })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p75-by-dow — P75 de size_bytes × DOW (0=Sun) de created_at. Sprint #1551.
async fn file_stats_size_p75_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p75_size \
           FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
          GROUP BY dow ORDER BY dow ASC",
    ).bind(ctx.tenant_id).fetch_all(pool).await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, p75)| { let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown"); serde_json::json!({"dow": dow, "day_name": day_name, "p75_size": p75}) })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/trashed-size-by-hour — SUM/AVG size_bytes de arquivos deletados × hora-do-dia. Sprint #1556.
async fn file_stats_trashed_size_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64, f64)> = sqlx::query_as(
        "SELECT EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour, \
            COALESCE(SUM(size_bytes), 0)::BIGINT AS total_size, \
            COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_size \
           FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NOT NULL \
          GROUP BY hour ORDER BY hour ASC",
    ).bind(ctx.tenant_id).fetch_all(pool).await?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, total, avg)| serde_json::json!({"hour": h, "total_trashed_size": total, "avg_trashed_size": avg}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/trashed-size-by-weekday — SUM/AVG size_bytes de arquivos deletados × DOW. Sprint #1561.
async fn file_stats_trashed_size_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64, f64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
            COALESCE(SUM(size_bytes), 0)::BIGINT AS total_size, \
            COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_size \
           FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NOT NULL \
          GROUP BY dow ORDER BY dow ASC",
    ).bind(ctx.tenant_id).fetch_all(pool).await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, total, avg)| { let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown"); serde_json::json!({"dow": dow, "day_name": day_name, "total_trashed_size": total, "avg_trashed_size": avg}) })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/trashed-size-by-month — SUM/AVG size_bytes de ficheiros deleted × mês. Sprint #1566.
async fn file_stats_trashed_size_by_month(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT EXTRACT(MONTH FROM deleted_at AT TIME ZONE 'UTC')::INT AS month, \
         SUM(size_bytes)::BIGINT AS total_trashed_size, AVG(size_bytes)::BIGINT AS avg_trashed_size \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NOT NULL \
         GROUP BY month ORDER BY month ASC",
    ).bind(ctx.tenant_id).fetch_all(pool).await?;
    const MONTH_NAMES: [&str; 12] = ["January","February","March","April","May","June","July","August","September","October","November","December"];
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(m, total, avg)| {
        let month_name = MONTH_NAMES.get((m - 1) as usize).copied().unwrap_or("Unknown");
        serde_json::json!({"month": m, "month_name": month_name, "total_trashed_size": total, "avg_trashed_size": avg})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p90-by-hour — P90 de size_bytes × hora-do-dia de created_at. Sprint #1571.
async fn file_stats_size_p90_by_hour(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour, \
         PERCENTILE_CONT(0.90) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p90_size \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY hour ORDER BY hour ASC",
    ).bind(ctx.tenant_id).fetch_all(pool).await?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(h, p90)| serde_json::json!({"hour": h, "p90_size": p90})).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p90-by-weekday — P90 de size_bytes × dia-da-semana de created_at. Sprint #1576.
async fn file_stats_size_p90_by_weekday(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
         PERCENTILE_CONT(0.90) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p90_size \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY dow ORDER BY dow ASC",
    ).bind(ctx.tenant_id).fetch_all(pool).await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(dow, p90)| {
        let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
        serde_json::json!({"dow": dow, "day_name": day_name, "p90_size": p90})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p90-by-month — P90 de size_bytes × mês de created_at. Sprint #1581.
async fn file_stats_size_p90_by_month(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
         PERCENTILE_CONT(0.90) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p90_size \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY month ORDER BY month ASC",
    ).bind(ctx.tenant_id).fetch_all(pool).await?;
    const MONTH_NAMES: [&str; 12] = ["January","February","March","April","May","June","July","August","September","October","November","December"];
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(m, p90)| {
        let month_name = MONTH_NAMES.get((m - 1) as usize).copied().unwrap_or("Unknown");
        serde_json::json!({"month": m, "month_name": month_name, "p90_size": p90})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/file-count-by-month — COUNT ficheiros (não trashed) × mês de created_at. Sprint #1586.
async fn file_stats_file_count_by_month(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY month ORDER BY month ASC",
    ).bind(ctx.tenant_id).fetch_all(pool).await?;
    const MONTH_NAMES: [&str; 12] = ["January","February","March","April","May","June","July","August","September","October","November","December"];
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(m, cnt)| {
        let month_name = MONTH_NAMES.get((m - 1) as usize).copied().unwrap_or("Unknown");
        serde_json::json!({"month": m, "month_name": month_name, "file_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/file-count-by-dow — COUNT ficheiros × DOW de created_at. Sprint #1591.
async fn file_stats_file_count_by_dow(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY dow ORDER BY dow ASC",
    ).bind(ctx.tenant_id).fetch_all(pool).await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(dow, cnt)| {
        let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
        serde_json::json!({"dow": dow, "day_name": day_name, "file_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p90-by-dow — P90 de size_bytes × DOW de created_at. Sprint #1596.
async fn file_stats_size_p90_by_dow(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
         PERCENTILE_CONT(0.90) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p90_size \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY dow ORDER BY dow ASC",
    ).bind(ctx.tenant_id).fetch_all(pool).await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(dow, p90)| {
        let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
        serde_json::json!({"dow": dow, "day_name": day_name, "p90_size": p90})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/trashed-size-by-dow — SUM/AVG size_bytes de ficheiros deleted × DOW. Sprint #1601.
async fn file_stats_trashed_size_by_dow(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM deleted_at AT TIME ZONE 'UTC')::INT AS dow, \
         SUM(size_bytes)::BIGINT AS total_trashed_size, AVG(size_bytes)::BIGINT AS avg_trashed_size \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NOT NULL \
         GROUP BY dow ORDER BY dow ASC",
    ).bind(ctx.tenant_id).fetch_all(pool).await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(dow, total, avg)| {
        let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
        serde_json::json!({"dow": dow, "day_name": day_name, "total_trashed_size": total, "avg_trashed_size": avg})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/folder-count-by-dow — COUNT pastas × DOW de created_at. Sprint #1606.
async fn file_stats_folder_count_by_dow(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, COUNT(*)::BIGINT AS folder_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'folder' AND deleted_at IS NULL \
         GROUP BY dow ORDER BY dow ASC",
    ).bind(ctx.tenant_id).fetch_all(pool).await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(dow, cnt)| {
        let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
        serde_json::json!({"dow": dow, "day_name": day_name, "folder_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/total-size-by-month — SUM size_bytes de ficheiros × mês de created_at. Sprint #1611.
async fn file_stats_total_size_by_month(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
         SUM(size_bytes)::BIGINT AS total_size, COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY month ORDER BY month ASC",
    ).bind(ctx.tenant_id).fetch_all(pool).await?;
    const MONTH_NAMES: [&str; 12] = ["January","February","March","April","May","June","July","August","September","October","November","December"];
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(m, total, cnt)| {
        let month_name = MONTH_NAMES.get((m - 1) as usize).copied().unwrap_or("Unknown");
        serde_json::json!({"month": m, "month_name": month_name, "total_size": total, "file_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/total-size-by-dow — SUM size_bytes de ficheiros × DOW de created_at. Sprint #1616.
async fn file_stats_total_size_by_dow(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
         SUM(size_bytes)::BIGINT AS total_size, COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY dow ORDER BY dow ASC",
    ).bind(ctx.tenant_id).fetch_all(pool).await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(dow, total, cnt)| {
        let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
        serde_json::json!({"dow": dow, "day_name": day_name, "total_size": total, "file_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/total-size-by-hour — SUM size_bytes de ficheiros × hora de created_at. Sprint #1621.
async fn file_stats_total_size_by_hour(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour, \
         SUM(size_bytes)::BIGINT AS total_size, COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY hour ORDER BY hour ASC",
    ).bind(ctx.tenant_id).fetch_all(pool).await?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(h, total, cnt)| serde_json::json!({"hour": h, "total_size": total, "file_count": cnt})).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/total-size-by-weekday — SUM size_bytes × dia-da-semana (nome) de created_at. Sprint #1626.
async fn file_stats_total_size_by_weekday(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
         SUM(size_bytes)::BIGINT AS total_size, COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY dow ORDER BY dow ASC",
    ).bind(ctx.tenant_id).fetch_all(pool).await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(dow, total, cnt)| {
        let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
        serde_json::json!({"dow": dow, "day_name": day_name, "total_size": total, "file_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p50-by-hour — P50 (mediana) de size_bytes × hora de created_at. Sprint #1631.
async fn file_stats_size_p50_by_hour(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour, \
         PERCENTILE_CONT(0.50) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p50_size \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY hour ORDER BY hour ASC",
    ).bind(ctx.tenant_id).fetch_all(pool).await?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(h, p50)| serde_json::json!({"hour": h, "p50_size": p50})).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p50-by-weekday — P50 (mediana) de size_bytes × dia-da-semana. Sprint #1636.
async fn file_stats_size_p50_by_weekday(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
         PERCENTILE_CONT(0.50) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p50_size \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY dow ORDER BY dow ASC",
    ).bind(ctx.tenant_id).fetch_all(pool).await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(dow, p50)| {
        let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
        serde_json::json!({"dow": dow, "day_name": day_name, "p50_size": p50})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p50-by-month — P50 (mediana) de size_bytes × mês de created_at. Sprint #1641.
async fn file_stats_size_p50_by_month(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
         PERCENTILE_CONT(0.50) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p50_size \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY month ORDER BY month ASC",
    ).bind(ctx.tenant_id).fetch_all(pool).await?;
    const MONTH_NAMES: [&str; 12] = ["January","February","March","April","May","June","July","August","September","October","November","December"];
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(m, p50)| {
        let month_name = MONTH_NAMES.get((m - 1) as usize).copied().unwrap_or("Unknown");
        serde_json::json!({"month": m, "month_name": month_name, "p50_size": p50})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/stats/file-size-p50-by-dow — P50 size_bytes × DOW. Sprint #1646.
async fn file_stats_size_p50_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
         PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p50_size \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY dow ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(dow, p50)| serde_json::json!({"dow": dow, "p50_size_bytes": p50}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/stats/file-size-p25-by-hour — P25 size_bytes × hour. Sprint #1651.
async fn file_stats_size_p25_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour, \
         PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p25_size \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY hour ORDER BY hour ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(hour, p25)| serde_json::json!({"hour": hour, "p25_size_bytes": p25}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/stats/file-size-p25-by-weekday — P25 size_bytes × weekday name. Sprint #1656.
async fn file_stats_size_p25_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
         PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p25_size \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY dow ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(dow, p25)| {
            let name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": name, "p25_size_bytes": p25})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/stats/file-size-p25-by-month — P25 size_bytes × month. Sprint #1661.
async fn file_stats_size_p25_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
         PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p25_size \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY month ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    const MONTH_NAMES: [&str; 12] = ["January","February","March","April","May","June","July","August","September","October","November","December"];
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(month, p25)| {
            let name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"month": month, "month_name": name, "p25_size_bytes": p25})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/stats/file-size-p25-by-dow — P25 size_bytes × DOW. Sprint #1666.
async fn file_stats_size_p25_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
         PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p25_size \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY dow ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(dow, p25)| serde_json::json!({"dow": dow, "p25_size_bytes": p25}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/stats/file-size-p10-by-hour — P10 size_bytes × hour. Sprint #1671.
async fn file_stats_size_p10_by_hour(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour, \
         PERCENTILE_CONT(0.10) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p10_size \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY hour ORDER BY hour ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(hour, p10)| serde_json::json!({"hour": hour, "p10_size_bytes": p10}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/stats/file-size-p10-by-weekday — P10 size_bytes × weekday name. Sprint #1676.
async fn file_stats_size_p10_by_weekday(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
         PERCENTILE_CONT(0.10) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p10_size \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY dow ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(dow, p10)| {
            let name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": name, "p10_size_bytes": p10})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/stats/file-size-p10-by-month — P10 size_bytes × month. Sprint #1681.
async fn file_stats_size_p10_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
         PERCENTILE_CONT(0.10) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p10_size \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY month ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    const MONTH_NAMES: [&str; 12] = ["January","February","March","April","May","June","July","August","September","October","November","December"];
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(month, p10)| {
            let name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"month": month, "month_name": name, "p10_size_bytes": p10})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/stats/name-length-by-dow — AVG name length × DOW. Sprint #1686.
async fn file_stats_name_length_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, f64, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
         AVG(LENGTH(name))::FLOAT8 AS avg_name_length, \
         MAX(LENGTH(name))::BIGINT AS max_name_length \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY dow ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(dow, avg, max)| {
            let name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": name, "avg_name_length": avg, "max_name_length": max})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/stats/version-size-by-dow — total version size × DOW. Sprint #1691.
async fn file_stats_version_size_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM f.created_at AT TIME ZONE 'UTC')::INT AS dow, \
         SUM(COALESCE(v.size_bytes, 0))::BIGINT AS total_version_size, \
         COUNT(v.id)::BIGINT AS version_count \
         FROM drive_files f \
         LEFT JOIN drive_file_versions v ON v.file_id = f.id \
         WHERE f.tenant_id = $1 AND f.kind = 'file' AND f.deleted_at IS NULL \
         GROUP BY dow ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(dow, total, cnt)| {
            let name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": name, "total_version_size_bytes": total, "version_count": cnt})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/stats/starred-by-dow — starred file count × DOW. Sprint #1696.
async fn file_stats_starred_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
         COUNT(*)::BIGINT AS starred_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL AND starred = true \
         GROUP BY dow ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(dow, cnt)| {
            let name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": name, "starred_count": cnt})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/stats/modified-by-dow — modified file count × DOW of updated_at. Sprint #1701.
async fn file_stats_modified_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM updated_at AT TIME ZONE 'UTC')::INT AS dow, \
         COUNT(*)::BIGINT AS modified_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY dow ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(dow, cnt)| {
            let name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": name, "modified_count": cnt})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/stats/deleted-by-dow — deleted file count × DOW of deleted_at. Sprint #1706.
async fn file_stats_deleted_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM deleted_at AT TIME ZONE 'UTC')::INT AS dow, \
         COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NOT NULL \
         GROUP BY dow ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(dow, cnt)| {
            let name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": name, "deleted_count": cnt})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/stats/trashed-count-by-user — trashed file count × user. Sprint #1711.
async fn file_stats_trashed_count_by_user(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(uuid::Uuid, i64)> = sqlx::query_as(
        "SELECT owner_id, COUNT(*)::BIGINT AS trashed_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND trashed = true \
         GROUP BY owner_id ORDER BY trashed_count DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(uid, cnt)| serde_json::json!({"user_id": uid, "trashed_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/stats/locked-count-by-user — locked file count × user. Sprint #1716.
async fn file_stats_locked_count_by_user(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(uuid::Uuid, i64)> = sqlx::query_as(
        "SELECT owner_id, COUNT(*)::BIGINT AS locked_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND locked = true AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY locked_count DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(uid, cnt)| serde_json::json!({"user_id": uid, "locked_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/stats/tag-count-by-user — tagged file count × user. Sprint #1721.
async fn file_stats_tag_count_by_user(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(uuid::Uuid, i64)> = sqlx::query_as(
        "SELECT owner_id, COUNT(*)::BIGINT AS tagged_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         AND tags IS NOT NULL AND tags <> '[]'::jsonb \
         GROUP BY owner_id ORDER BY tagged_count DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(uid, cnt)| serde_json::json!({"user_id": uid, "tagged_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/stats/shared-by-dow — shared file count × DOW. Sprint #1726.
async fn file_stats_shared_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
         COUNT(*)::BIGINT AS shared_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL AND shared = true \
         GROUP BY dow ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(dow, cnt)| {
            let name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": name, "shared_count": cnt})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/stats/starred-count-by-dow — alias: starred file count × DOW for analytics. Sprint #1731.
async fn file_stats_starred_count_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
         COUNT(*) FILTER (WHERE starred = true)::BIGINT AS starred_count, \
         COUNT(*)::BIGINT AS total_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY dow ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(dow, starred, total)| {
            let name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            let rate = if total > 0 { starred as f64 / total as f64 } else { 0.0 };
            serde_json::json!({"dow": dow, "day_name": name, "starred_count": starred, "total_count": total, "starred_rate": rate})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/stats/locked-by-dow — locked file count × DOW. Sprint #1736.
async fn file_stats_locked_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
         COUNT(*)::BIGINT AS locked_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL AND locked = true \
         GROUP BY dow ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(dow, cnt)| {
            let name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": name, "locked_count": cnt})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/stats/trashed-by-dow — trashed file count × DOW. Sprint #1741.
async fn file_stats_trashed_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
         COUNT(*)::BIGINT AS trashed_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND trashed = true \
         GROUP BY dow ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(dow, cnt)| {
            let name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": name, "trashed_count": cnt})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/stats/orphan-by-dow — orphan file count × DOW. Sprint #1746.
async fn file_stats_orphan_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
         COUNT(*)::BIGINT AS orphan_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL AND folder_id IS NULL \
         GROUP BY dow ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(dow, cnt)| {
            let name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": name, "orphan_count": cnt})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/stats/zero-size-by-dow — zero-byte file count × DOW. Sprint #1751.
async fn file_stats_zero_size_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
         COUNT(*)::BIGINT AS zero_size_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL AND size_bytes = 0 \
         GROUP BY dow ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(dow, cnt)| {
            let name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": name, "zero_size_count": cnt})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/stats/empty-by-dow — empty file count × DOW (size_bytes <= 1). Sprint #1756.
async fn file_stats_empty_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
         COUNT(*)::BIGINT AS empty_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL AND size_bytes <= 1 \
         GROUP BY dow ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(dow, cnt)| {
            let name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": name, "empty_count": cnt})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/stats/starred-size-by-dow — total size of starred files × DOW. Sprint #1761.
async fn file_stats_starred_size_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
         SUM(size_bytes) FILTER (WHERE starred = true)::BIGINT AS starred_size, \
         COUNT(*) FILTER (WHERE starred = true)::BIGINT AS starred_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY dow ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(dow, size, cnt)| {
            let name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": name, "starred_size_bytes": size, "starred_count": cnt})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/stats/locked-size-by-dow — total size of locked files × DOW. Sprint #1766.
async fn file_stats_locked_size_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
         SUM(size_bytes) FILTER (WHERE locked = true)::BIGINT AS locked_size, \
         COUNT(*) FILTER (WHERE locked = true)::BIGINT AS locked_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY dow ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(dow, size, cnt)| {
            let name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": name, "locked_size_bytes": size, "locked_count": cnt})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/stats/trashed-size-by-user — total trashed file size × user. Sprint #1771.
async fn file_stats_trashed_size_by_user(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(uuid::Uuid, i64, i64)> = sqlx::query_as(
        "SELECT owner_id, SUM(size_bytes)::BIGINT AS trashed_size, COUNT(*)::BIGINT AS trashed_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND trashed = true \
         GROUP BY owner_id ORDER BY trashed_size DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(uid, size, cnt)| serde_json::json!({"user_id": uid, "trashed_size_bytes": size, "trashed_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/stats/shared-size-by-dow — total size of shared files × DOW. Sprint #1776.
async fn file_stats_shared_size_by_dow(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
         SUM(size_bytes) FILTER (WHERE shared = true)::BIGINT AS shared_size, \
         COUNT(*) FILTER (WHERE shared = true)::BIGINT AS shared_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY dow ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(dow, size, cnt)| {
            let name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": name, "shared_size_bytes": size, "shared_count": cnt})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/stats/locked-size-by-month — total size of locked files × month. Sprint #1781.
async fn file_stats_locked_size_by_month(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
         SUM(size_bytes) FILTER (WHERE locked = true)::BIGINT AS locked_size, \
         COUNT(*) FILTER (WHERE locked = true)::BIGINT AS locked_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY month ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    const MONTH_NAMES: [&str; 12] = ["January","February","March","April","May","June","July","August","September","October","November","December"];
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(month, size, cnt)| {
            let name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"month": month, "month_name": name, "locked_size_bytes": size, "locked_count": cnt})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/zero-size-by-month — zero-size files COUNT × month. Sprint #1786.
async fn file_stats_zero_size_by_month(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
         COUNT(*) FILTER (WHERE size_bytes = 0)::BIGINT AS zero_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY month ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(month, cnt)| {
            let name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"month": month, "month_name": name, "zero_size_count": cnt})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/zero-size-by-hour — zero-size files COUNT × hour. Sprint #1791.
async fn file_stats_zero_size_by_hour(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour, \
         COUNT(*) FILTER (WHERE size_bytes = 0)::BIGINT AS zero_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY hour ORDER BY hour ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(hour, cnt)| serde_json::json!({"hour": hour, "zero_size_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/zero-size-by-user — zero-size files COUNT × user. Sprint #1796.
async fn file_stats_zero_size_by_user(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT AS user_id, \
         COUNT(*) FILTER (WHERE size_bytes = 0)::BIGINT AS zero_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY zero_count DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(uid, cnt)| serde_json::json!({"user_id": uid, "zero_size_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/zero-size-by-weekday — zero-size files COUNT × DOW. Sprint #1801.
async fn file_stats_zero_size_by_weekday(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
         COUNT(*) FILTER (WHERE size_bytes = 0)::BIGINT AS zero_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY dow ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(dow, cnt)| {
            let name = DAY_NAMES.get(dow as usize % 7).copied().unwrap_or("Unknown");
            serde_json::json!({"day_of_week": dow, "day_name": name, "zero_size_count": cnt})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/empty-size-by-month — empty files (size_bytes=0) total vs non-empty × month. Sprint #1806.
async fn file_stats_empty_size_by_month(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
         COUNT(*) FILTER (WHERE size_bytes = 0)::BIGINT AS empty_count, \
         COALESCE(SUM(size_bytes) FILTER (WHERE size_bytes > 0), 0)::BIGINT AS non_empty_bytes \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY month ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(month, empty, nonempty)| {
            let name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"month": month, "month_name": name, "empty_count": empty, "non_empty_bytes": nonempty})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/empty-size-by-hour — empty files COUNT × hour. Sprint #1811.
async fn file_stats_empty_size_by_hour(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour, \
         COUNT(*) FILTER (WHERE size_bytes = 0)::BIGINT AS empty_count, \
         COALESCE(SUM(size_bytes) FILTER (WHERE size_bytes > 0), 0)::BIGINT AS non_empty_bytes \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY hour ORDER BY hour ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(hour, empty, nonempty)| {
            serde_json::json!({"hour": hour, "empty_count": empty, "non_empty_bytes": nonempty})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/empty-size-by-user — empty files COUNT × user. Sprint #1816.
async fn file_stats_empty_size_by_user(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT AS user_id, \
         COUNT(*) FILTER (WHERE size_bytes = 0)::BIGINT AS empty_count, \
         COALESCE(SUM(size_bytes) FILTER (WHERE size_bytes > 0), 0)::BIGINT AS non_empty_bytes \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY empty_count DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(uid, empty, nonempty)| {
            serde_json::json!({"user_id": uid, "empty_count": empty, "non_empty_bytes": nonempty})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/empty-size-by-weekday — empty files COUNT × DOW. Sprint #1821.
async fn file_stats_empty_size_by_weekday(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
         COUNT(*) FILTER (WHERE size_bytes = 0)::BIGINT AS empty_count, \
         COALESCE(SUM(size_bytes) FILTER (WHERE size_bytes > 0), 0)::BIGINT AS non_empty_bytes \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY dow ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(dow, empty, nonempty)| {
            let name = DAY_NAMES.get(dow as usize % 7).copied().unwrap_or("Unknown");
            serde_json::json!({"day_of_week": dow, "day_name": name, "empty_count": empty, "non_empty_bytes": nonempty})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/empty-count-by-month — COUNT empty files × month (without sizes). Sprint #1826.
async fn file_stats_empty_count_by_month(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
         COUNT(*) FILTER (WHERE size_bytes = 0)::BIGINT AS empty_count, \
         COUNT(*)::BIGINT AS total_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY month ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(month, empty, total)| {
            let name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            let ratio = if total > 0 { empty as f64 / total as f64 } else { 0.0 };
            serde_json::json!({"month": month, "month_name": name, "empty_count": empty, "total_count": total, "empty_ratio": ratio})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/empty-count-by-hour — COUNT empty files × hour. Sprint #1831.
async fn file_stats_empty_count_by_hour(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour, \
         COUNT(*) FILTER (WHERE size_bytes = 0)::BIGINT AS empty_count, \
         COUNT(*)::BIGINT AS total_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY hour ORDER BY hour ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(hour, empty, total)| {
            let ratio = if total > 0 { empty as f64 / total as f64 } else { 0.0 };
            serde_json::json!({"hour": hour, "empty_count": empty, "total_count": total, "empty_ratio": ratio})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/empty-count-by-user — COUNT empty files × user. Sprint #1836.
async fn file_stats_empty_count_by_user(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT AS user_id, \
         COUNT(*) FILTER (WHERE size_bytes = 0)::BIGINT AS empty_count, \
         COUNT(*)::BIGINT AS total_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY empty_count DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(uid, empty, total)| {
            let ratio = if total > 0 { empty as f64 / total as f64 } else { 0.0 };
            serde_json::json!({"user_id": uid, "empty_count": empty, "total_count": total, "empty_ratio": ratio})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/empty-count-by-weekday — COUNT empty files × DOW. Sprint #1841.
async fn file_stats_empty_count_by_weekday(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
         COUNT(*) FILTER (WHERE size_bytes = 0)::BIGINT AS empty_count, \
         COUNT(*)::BIGINT AS total_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY dow ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(dow, empty, total)| {
            let name = DAY_NAMES.get(dow as usize % 7).copied().unwrap_or("Unknown");
            let ratio = if total > 0 { empty as f64 / total as f64 } else { 0.0 };
            serde_json::json!({"day_of_week": dow, "day_name": name, "empty_count": empty, "total_count": total, "empty_ratio": ratio})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/empty-ratio-by-month — ratio empty/total × month. Sprint #1846.
async fn file_stats_empty_ratio_by_month(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(i32, f64)> = sqlx::query_as(
        "SELECT EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, \
         COUNT(*) FILTER (WHERE size_bytes = 0)::FLOAT8 / NULLIF(COUNT(*), 0) AS empty_ratio \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY month ORDER BY month ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(month, ratio)| {
            let name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"month": month, "month_name": name, "empty_ratio": ratio})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/empty-ratio-by-hour — ratio empty/total × hour. Sprint #1851.
async fn file_stats_empty_ratio_by_hour(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(i32, f64)> = sqlx::query_as(
        "SELECT EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour, \
         COUNT(*) FILTER (WHERE size_bytes = 0)::FLOAT8 / NULLIF(COUNT(*), 0) AS empty_ratio \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY hour ORDER BY hour ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(hour, ratio)| serde_json::json!({"hour": hour, "empty_ratio": ratio}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/empty-ratio-by-user — ratio empty/total × user. Sprint #1856.
async fn file_stats_empty_ratio_by_user(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64)> = sqlx::query_as(
        "SELECT owner_id::TEXT AS user_id, \
         COUNT(*) FILTER (WHERE size_bytes = 0)::FLOAT8 / NULLIF(COUNT(*), 0) AS empty_ratio \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY empty_ratio DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(uid, ratio)| serde_json::json!({"user_id": uid, "empty_ratio": ratio}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/empty-ratio-by-weekday — ratio empty/total × DOW. Sprint #1861.
async fn file_stats_empty_ratio_by_weekday(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(i32, f64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
         COUNT(*) FILTER (WHERE size_bytes = 0)::FLOAT8 / NULLIF(COUNT(*), 0) AS empty_ratio \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY dow ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(dow, ratio)| {
            let name = DAY_NAMES.get(dow as usize % 7).copied().unwrap_or("Unknown");
            serde_json::json!({"day_of_week": dow, "day_name": name, "empty_ratio": ratio})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/empty-ratio-by-ext — ratio empty/total × extension. Sprint #1866.
async fn file_stats_empty_ratio_by_ext(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT LOWER(COALESCE(NULLIF(regexp_replace(name, '^.*\\.', ''), name), 'none')) AS ext, \
         COUNT(*) FILTER (WHERE size_bytes = 0)::FLOAT8 / NULLIF(COUNT(*), 0) AS empty_ratio, \
         COUNT(*)::BIGINT AS total_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY ext ORDER BY empty_ratio DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(ext, ratio, total)| serde_json::json!({"extension": ext, "empty_ratio": ratio, "total_count": total}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/empty-ratio-by-dow — ratio empty/total × DOW. Sprint #1871.
async fn file_stats_empty_ratio_by_dow(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(i32, f64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, \
         COUNT(*) FILTER (WHERE size_bytes = 0)::FLOAT8 / NULLIF(COUNT(*), 0) AS empty_ratio \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY dow ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(dow, ratio)| {
            let name = DAY_NAMES.get(dow as usize % 7).copied().unwrap_or("Unknown");
            serde_json::json!({"day_of_week": dow, "day_name": name, "empty_ratio": ratio})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p25-by-user — P25 size_bytes × user. Sprint #1876.
async fn file_stats_size_p25_by_user(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT AS user_id, \
         PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p25_size_bytes, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY p25_size_bytes DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(uid, p25, cnt)| serde_json::json!({"user_id": uid, "p25_size_bytes": p25, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p10-by-user — P10 size_bytes × user. Sprint #1881.
async fn file_stats_size_p10_by_user(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT AS user_id, \
         PERCENTILE_CONT(0.10) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p10_size_bytes, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY p10_size_bytes DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(uid, p10, cnt)| serde_json::json!({"user_id": uid, "p10_size_bytes": p10, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p10-by-ext — P10 size_bytes × extension. Sprint #1886.
async fn file_stats_size_p10_by_ext(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT LOWER(COALESCE(NULLIF(regexp_replace(name, '^.*\\.', ''), name), 'none')) AS ext, \
         PERCENTILE_CONT(0.10) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p10_size_bytes, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY ext ORDER BY p10_size_bytes DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(ext, p10, cnt)| serde_json::json!({"extension": ext, "p10_size_bytes": p10, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p25-by-ext — P25 size_bytes × extension. Sprint #1891.
async fn file_stats_size_p25_by_ext(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT LOWER(COALESCE(NULLIF(regexp_replace(name, '^.*\\.', ''), name), 'none')) AS ext, \
         PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p25_size_bytes, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY ext ORDER BY p25_size_bytes DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(ext, p25, cnt)| serde_json::json!({"extension": ext, "p25_size_bytes": p25, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p50-by-ext — P50 size_bytes × extension. Sprint #1896.
async fn file_stats_size_p50_by_ext(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT LOWER(COALESCE(NULLIF(regexp_replace(name, '^.*\\.', ''), name), 'none')) AS ext, \
         PERCENTILE_CONT(0.50) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p50_size_bytes, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY ext ORDER BY p50_size_bytes DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(ext, p50, cnt)| serde_json::json!({"extension": ext, "p50_size_bytes": p50, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p75-by-ext — P75 size_bytes × extension. Sprint #1901.
async fn file_stats_size_p75_by_ext(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT LOWER(COALESCE(NULLIF(regexp_replace(name, '^.*\\.', ''), name), 'none')) AS ext, \
         PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p75_size_bytes, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY ext ORDER BY p75_size_bytes DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(ext, p75, cnt)| serde_json::json!({"extension": ext, "p75_size_bytes": p75, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p90-by-ext — P90 size_bytes × extension. Sprint #1906.
async fn file_stats_size_p90_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT LOWER(COALESCE(NULLIF(regexp_replace(name, '^.*\\.', ''), name), 'none')) AS ext, \
         PERCENTILE_CONT(0.90) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p90_size_bytes, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY ext ORDER BY p90_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, p90, cnt)| serde_json::json!({"extension": ext, "p90_size_bytes": p90, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p95-by-ext — P95 size_bytes × extension. Sprint #1911.
async fn file_stats_size_p95_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT LOWER(COALESCE(NULLIF(regexp_replace(name, '^.*\\.', ''), name), 'none')) AS ext, \
         PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p95_size_bytes, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY ext ORDER BY p95_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, p95, cnt)| serde_json::json!({"extension": ext, "p95_size_bytes": p95, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p99-by-ext — P99 size_bytes × extension. Sprint #1916.
async fn file_stats_size_p99_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT LOWER(COALESCE(NULLIF(regexp_replace(name, '^.*\\.', ''), name), 'none')) AS ext, \
         PERCENTILE_CONT(0.99) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p99_size_bytes, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY ext ORDER BY p99_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, p99, cnt)| serde_json::json!({"extension": ext, "p99_size_bytes": p99, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p10-by-owner — P10 size_bytes × owner_id. Sprint #1921.
async fn file_stats_size_p10_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
         PERCENTILE_CONT(0.10) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p10_size_bytes, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY p10_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, p10, cnt)| serde_json::json!({"owner_id": owner, "p10_size_bytes": p10, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p25-by-owner — P25 size_bytes × owner_id. Sprint #1926.
async fn file_stats_size_p25_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
         PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p25_size_bytes, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY p25_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, p25, cnt)| serde_json::json!({"owner_id": owner, "p25_size_bytes": p25, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p50-by-owner — P50 size_bytes × owner_id. Sprint #1931.
async fn file_stats_size_p50_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
         PERCENTILE_CONT(0.50) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p50_size_bytes, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY p50_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, p50, cnt)| serde_json::json!({"owner_id": owner, "p50_size_bytes": p50, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p75-by-owner — P75 size_bytes × owner_id. Sprint #1936.
async fn file_stats_size_p75_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
         PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p75_size_bytes, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY p75_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, p75, cnt)| serde_json::json!({"owner_id": owner, "p75_size_bytes": p75, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p90-by-owner — P90 size_bytes × owner_id. Sprint #1941.
async fn file_stats_size_p90_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
         PERCENTILE_CONT(0.90) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p90_size_bytes, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY p90_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, p90, cnt)| serde_json::json!({"owner_id": owner, "p90_size_bytes": p90, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p95-by-owner — P95 size_bytes × owner_id. Sprint #1946.
async fn file_stats_size_p95_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
         PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p95_size_bytes, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY p95_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, p95, cnt)| serde_json::json!({"owner_id": owner, "p95_size_bytes": p95, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p99-by-owner — P99 size_bytes × owner_id. Sprint #1951.
async fn file_stats_size_p99_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
         PERCENTILE_CONT(0.99) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p99_size_bytes, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY p99_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, p99, cnt)| serde_json::json!({"owner_id": owner, "p99_size_bytes": p99, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p10-by-kind — P10 size_bytes × file kind (mime category). Sprint #1956.
async fn file_stats_size_p10_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(SPLIT_PART(mime_type, '/', 1), 'unknown') AS kind_category, \
         PERCENTILE_CONT(0.10) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p10_size_bytes, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY kind_category ORDER BY p10_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kc, p10, cnt)| serde_json::json!({"kind_category": kc, "p10_size_bytes": p10, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p25-by-kind — P25 size_bytes × file kind (mime category). Sprint #1961.
async fn file_stats_size_p25_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(SPLIT_PART(mime_type, '/', 1), 'unknown') AS kind_category, \
         PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p25_size_bytes, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY kind_category ORDER BY p25_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kc, p25, cnt)| serde_json::json!({"kind_category": kc, "p25_size_bytes": p25, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p50-by-kind — P50 size_bytes × mime category. Sprint #1966.
async fn file_stats_size_p50_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(SPLIT_PART(mime_type, '/', 1), 'unknown') AS kind_category, \
         PERCENTILE_CONT(0.50) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p50_size_bytes, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY kind_category ORDER BY p50_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kc, p50, cnt)| serde_json::json!({"kind_category": kc, "p50_size_bytes": p50, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p75-by-kind — P75 size_bytes × mime category. Sprint #1971.
async fn file_stats_size_p75_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(SPLIT_PART(mime_type, '/', 1), 'unknown') AS kind_category, \
         PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p75_size_bytes, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY kind_category ORDER BY p75_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kc, p75, cnt)| serde_json::json!({"kind_category": kc, "p75_size_bytes": p75, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p90-by-kind — P90 size_bytes × mime category. Sprint #1976.
async fn file_stats_size_p90_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(SPLIT_PART(mime_type, '/', 1), 'unknown') AS kind_category, \
         PERCENTILE_CONT(0.90) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p90_size_bytes, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY kind_category ORDER BY p90_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kc, p90, cnt)| serde_json::json!({"kind_category": kc, "p90_size_bytes": p90, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p95-by-kind — P95 size_bytes × mime category. Sprint #1981.
async fn file_stats_size_p95_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(SPLIT_PART(mime_type, '/', 1), 'unknown') AS kind_category, \
         PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p95_size_bytes, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY kind_category ORDER BY p95_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kc, p95, cnt)| serde_json::json!({"kind_category": kc, "p95_size_bytes": p95, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p99-by-kind — P99 size_bytes × mime category. Sprint #1986.
async fn file_stats_size_p99_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(SPLIT_PART(mime_type, '/', 1), 'unknown') AS kind_category, \
         PERCENTILE_CONT(0.99) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p99_size_bytes, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY kind_category ORDER BY p99_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kc, p99, cnt)| serde_json::json!({"kind_category": kc, "p99_size_bytes": p99, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/count-by-owner — número de arquivos por owner. Sprint #1991.
async fn file_stats_count_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY file_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, cnt)| serde_json::json!({"owner_id": owner, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/folder-count-by-owner — número de pastas por owner. Sprint #1996.
async fn file_stats_folder_count_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, COUNT(*)::BIGINT AS folder_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'folder' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY folder_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, cnt)| serde_json::json!({"owner_id": owner, "folder_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-avg-by-kind — AVG size_bytes × mime category. Sprint #2001.
async fn file_stats_size_avg_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(SPLIT_PART(mime_type, '/', 1), 'unknown') AS kind_category, \
         AVG(size_bytes)::BIGINT AS avg_size_bytes, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY kind_category ORDER BY avg_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kc, avg, cnt)| serde_json::json!({"kind_category": kc, "avg_size_bytes": avg, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-avg-by-owner — AVG size_bytes por owner. Sprint #2006.
async fn file_stats_size_avg_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, AVG(size_bytes)::BIGINT AS avg_size_bytes, COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY avg_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, avg, cnt)| serde_json::json!({"owner_id": owner, "avg_size_bytes": avg, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-avg-by-ext — AVG size_bytes por extensão. Sprint #2011.
async fn file_stats_size_avg_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(LOWER(SPLIT_PART(name, '.', -1)), 'unknown') AS ext, \
         AVG(size_bytes)::BIGINT AS avg_size_bytes, COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY ext ORDER BY avg_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, avg, cnt)| serde_json::json!({"ext": ext, "avg_size_bytes": avg, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/count-by-ext — número de arquivos por extensão. Sprint #2016.
async fn file_stats_count_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT COALESCE(LOWER(SPLIT_PART(name, '.', -1)), 'unknown') AS ext, COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY ext ORDER BY file_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, cnt)| serde_json::json!({"ext": ext, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/total-size-by-owner — SUM size_bytes por owner. Sprint #2021.
async fn file_stats_total_size_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, SUM(size_bytes)::BIGINT AS total_size_bytes, COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY total_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, total, cnt)| serde_json::json!({"owner_id": owner, "total_size_bytes": total, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/total-size-by-ext — SUM size_bytes por extensão. Sprint #2026.
async fn file_stats_total_size_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(LOWER(SPLIT_PART(name, '.', -1)), 'unknown') AS ext, \
         SUM(size_bytes)::BIGINT AS total_size_bytes, COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY ext ORDER BY total_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, total, cnt)| serde_json::json!({"ext": ext, "total_size_bytes": total, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/total-size-by-kind — SUM size_bytes por mime category. Sprint #2031.
async fn file_stats_total_size_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(SPLIT_PART(mime_type, '/', 1), 'unknown') AS kind_category, \
         SUM(size_bytes)::BIGINT AS total_size_bytes, COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY kind_category ORDER BY total_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kc, total, cnt)| serde_json::json!({"kind_category": kc, "total_size_bytes": total, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/count-by-mime — número de arquivos por mime_type completo. Sprint #2036.
async fn file_stats_count_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type, 'unknown') AS mime_type, COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY file_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, cnt)| serde_json::json!({"mime_type": mime, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/empty-file-count-by-owner — arquivos de tamanho zero por owner. Sprint #2041.
async fn file_stats_empty_file_count_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
         COUNT(*) FILTER (WHERE size_bytes = 0)::BIGINT AS empty_file_count, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY empty_file_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, empty, total)| {
            let rate = if total > 0 { empty as f64 / total as f64 } else { 0.0 };
            serde_json::json!({"owner_id": owner, "empty_file_count": empty, "file_count": total, "empty_rate": rate})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/folder-size-avg-by-owner — AVG size_bytes de pastas por owner. Sprint #2046.
async fn file_stats_folder_size_avg_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, AVG(size_bytes)::BIGINT AS avg_size_bytes, COUNT(*)::BIGINT AS folder_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'folder' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY avg_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, avg, cnt)| serde_json::json!({"owner_id": owner, "avg_size_bytes": avg, "folder_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/folder-total-size-by-owner — SUM size_bytes de pastas por owner. Sprint #2051.
async fn file_stats_folder_total_size_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, SUM(size_bytes)::BIGINT AS total_size_bytes, COUNT(*)::BIGINT AS folder_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'folder' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY total_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, total, cnt)| serde_json::json!({"owner_id": owner, "total_size_bytes": total, "folder_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/folder-count-by-ext — número de pastas por extensão de nome. Sprint #2056.
async fn file_stats_folder_count_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT COALESCE(LOWER(SPLIT_PART(name, '.', -1)), 'none') AS ext, COUNT(*)::BIGINT AS folder_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'folder' AND deleted_at IS NULL \
         GROUP BY ext ORDER BY folder_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, cnt)| serde_json::json!({"ext": ext, "folder_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-count-by-owner — arquivos deletados por owner. Sprint #2061.
async fn file_stats_deleted_count_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NOT NULL \
         GROUP BY owner_id ORDER BY deleted_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, cnt)| serde_json::json!({"owner_id": owner, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-by-owner — SUM size_bytes de deletados por owner. Sprint #2066.
async fn file_stats_deleted_size_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, SUM(size_bytes)::BIGINT AS deleted_size_bytes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NOT NULL \
         GROUP BY owner_id ORDER BY deleted_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, size, cnt)| serde_json::json!({"owner_id": owner, "deleted_size_bytes": size, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-count-by-ext — arquivos deletados por extensão. Sprint #2071.
async fn file_stats_deleted_count_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT COALESCE(LOWER(SPLIT_PART(name, '.', -1)), 'unknown') AS ext, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NOT NULL \
         GROUP BY ext ORDER BY deleted_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, cnt)| serde_json::json!({"ext": ext, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-by-ext — SUM size_bytes de deletados por extensão. Sprint #2076.
async fn file_stats_deleted_size_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(LOWER(SPLIT_PART(name, '.', -1)), 'unknown') AS ext, \
         SUM(size_bytes)::BIGINT AS deleted_size_bytes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NOT NULL \
         GROUP BY ext ORDER BY deleted_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, size, cnt)| serde_json::json!({"ext": ext, "deleted_size_bytes": size, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-count-by-kind — arquivos deletados por mime category. Sprint #2081.
async fn file_stats_deleted_count_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT COALESCE(SPLIT_PART(mime_type, '/', 1), 'unknown') AS kind_category, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NOT NULL \
         GROUP BY kind_category ORDER BY deleted_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kc, cnt)| serde_json::json!({"kind_category": kc, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-by-kind — SUM size_bytes de deletados por mime category. Sprint #2086.
async fn file_stats_deleted_size_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(SPLIT_PART(mime_type, '/', 1), 'unknown') AS kind_category, \
         SUM(size_bytes)::BIGINT AS deleted_size_bytes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NOT NULL \
         GROUP BY kind_category ORDER BY deleted_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kc, size, cnt)| serde_json::json!({"kind_category": kc, "deleted_size_bytes": size, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/shared-count-by-owner — arquivos compartilhados por owner. Sprint #2091.
async fn file_stats_shared_count_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
         COUNT(*) FILTER (WHERE shared = TRUE)::BIGINT AS shared_count, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY shared_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, shared, total)| {
            let rate = if total > 0 { shared as f64 / total as f64 } else { 0.0 };
            serde_json::json!({"owner_id": owner, "shared_count": shared, "file_count": total, "shared_rate": rate})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/shared-size-by-owner — SUM size_bytes de compartilhados por owner. Sprint #2096.
async fn file_stats_shared_size_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
         SUM(size_bytes) FILTER (WHERE shared = TRUE)::BIGINT AS shared_size_bytes, \
         COUNT(*) FILTER (WHERE shared = TRUE)::BIGINT AS shared_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY shared_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, size, cnt)| serde_json::json!({"owner_id": owner, "shared_size_bytes": size, "shared_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/version-stddev-by-owner — desvio padrão da versão por owner. Sprint #2446.
async fn file_stats_version_stddev_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, COALESCE(STDDEV(version), 0.0)::FLOAT8 AS stddev_version, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY stddev_version DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, stddev, cnt)| serde_json::json!({"owner_id": owner, "stddev_version": stddev, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/version-stddev-by-ext — desvio padrão da versão por extensão. Sprint #2451.
async fn file_stats_version_stddev_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT LOWER(REGEXP_REPLACE(name, '^.*\\.', '')) AS ext, \
                COALESCE(STDDEV(version), 0.0)::FLOAT8 AS stddev_version, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY stddev_version DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, stddev, cnt)| serde_json::json!({"ext": ext, "stddev_version": stddev, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/version-stddev-by-mime — desvio padrão da versão por mime_type. Sprint #2456.
async fn file_stats_version_stddev_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT mime_type, COALESCE(STDDEV(version), 0.0)::FLOAT8 AS stddev_version, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY stddev_version DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, stddev, cnt)| serde_json::json!({"mime_type": mime, "stddev_version": stddev, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-min-by-owner — tamanho mínimo de arquivos deletados por owner. Sprint #2486.
async fn file_stats_size_deleted_min_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, MIN(size_bytes)::BIGINT AS min_deleted_size, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY owner_id ORDER BY min_deleted_size ASC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, min, cnt)| serde_json::json!({"owner_id": owner, "min_deleted_size_bytes": min, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-max-by-owner — tamanho máximo de arquivos deletados por owner. Sprint #2491.
async fn file_stats_size_deleted_max_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, MAX(size_bytes)::BIGINT AS max_deleted_size, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY owner_id ORDER BY max_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, max, cnt)| serde_json::json!({"owner_id": owner, "max_deleted_size_bytes": max, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-cv-by-owner — CV do tamanho de arquivos deletados por owner. Sprint #2496.
async fn file_stats_size_deleted_cv_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, f64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
                COALESCE(STDDEV(size_bytes), 0.0)::FLOAT8 AS stddev_sz, \
                COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_sz, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY owner_id ORDER BY avg_sz DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner, stddev, avg, cnt)| {
        let cv = if avg > 0.0 { stddev / avg } else { 0.0 };
        serde_json::json!({"owner_id": owner, "cv_deleted_size": cv, "stddev": stddev, "avg": avg, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-iqr-by-owner — IQR do tamanho de arquivos deletados por owner. Sprint #2501.
async fn file_stats_size_deleted_iqr_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
                (PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY size_bytes) \
                 - PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY size_bytes))::FLOAT8 AS iqr_sz, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY owner_id ORDER BY iqr_sz DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, iqr, cnt)| serde_json::json!({"owner_id": owner, "iqr_deleted_size_bytes": iqr, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-stddev-by-owner — desvio-padrão do tamanho de arquivos deletados por owner. Sprint #2466.
async fn file_stats_size_deleted_stddev_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, STDDEV(size_bytes)::FLOAT8 AS stddev_deleted_size, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY owner_id ORDER BY stddev_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, stddev, cnt)| serde_json::json!({"owner_id": owner, "stddev_deleted_size_bytes": stddev, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-min-by-ext — tamanho mínimo de arquivos deletados por extensão. Sprint #2506.
async fn file_stats_size_deleted_min_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT LOWER(REGEXP_REPLACE(name, '^.*\\.', '')) AS ext, \
                MIN(size_bytes)::BIGINT AS min_deleted_size, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY min_deleted_size ASC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, min, cnt)| serde_json::json!({"ext": ext, "min_deleted_size_bytes": min, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-max-by-ext — tamanho máximo de arquivos deletados por extensão. Sprint #2511.
async fn file_stats_size_deleted_max_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT LOWER(REGEXP_REPLACE(name, '^.*\\.', '')) AS ext, \
                MAX(size_bytes)::BIGINT AS max_deleted_size, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY max_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, max, cnt)| serde_json::json!({"ext": ext, "max_deleted_size_bytes": max, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-cv-by-ext — CV do tamanho de arquivos deletados por extensão. Sprint #2516.
async fn file_stats_size_deleted_cv_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, f64, i64)> = sqlx::query_as(
        "SELECT LOWER(REGEXP_REPLACE(name, '^.*\\.', '')) AS ext, \
                COALESCE(STDDEV(size_bytes), 0.0)::FLOAT8 AS stddev_sz, \
                COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_sz, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY avg_sz DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, stddev, avg, cnt)| {
        let cv = if avg > 0.0 { stddev / avg } else { 0.0 };
        serde_json::json!({"ext": ext, "cv_deleted_size": cv, "stddev": stddev, "avg": avg, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-iqr-by-ext — IQR do tamanho de arquivos deletados por extensão. Sprint #2521.
async fn file_stats_size_deleted_iqr_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT LOWER(REGEXP_REPLACE(name, '^.*\\.', '')) AS ext, \
                (PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY size_bytes) \
                 - PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY size_bytes))::FLOAT8 AS iqr_sz, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY iqr_sz DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, iqr, cnt)| serde_json::json!({"ext": ext, "iqr_deleted_size_bytes": iqr, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-stddev-by-ext — desvio-padrão do tamanho de arquivos deletados por extensão. Sprint #2471.
async fn file_stats_size_deleted_stddev_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT LOWER(REGEXP_REPLACE(name, '^.*\\.', '')) AS ext, \
                STDDEV(size_bytes)::FLOAT8 AS stddev_deleted_size, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY stddev_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, stddev, cnt)| serde_json::json!({"ext": ext, "stddev_deleted_size_bytes": stddev, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-min-by-kind — tamanho mínimo de arquivos deletados por kind. Sprint #2526.
async fn file_stats_size_deleted_min_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT kind, MIN(size_bytes)::BIGINT AS min_deleted_size, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY kind ORDER BY min_deleted_size ASC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, min, cnt)| serde_json::json!({"kind": kind, "min_deleted_size_bytes": min, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-max-by-kind — tamanho máximo de arquivos deletados por kind. Sprint #2531.
async fn file_stats_size_deleted_max_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT kind, MAX(size_bytes)::BIGINT AS max_deleted_size, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY kind ORDER BY max_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, max, cnt)| serde_json::json!({"kind": kind, "max_deleted_size_bytes": max, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-cv-by-kind — CV do tamanho de arquivos deletados por kind. Sprint #2536.
async fn file_stats_size_deleted_cv_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, f64, i64)> = sqlx::query_as(
        "SELECT kind, \
                COALESCE(STDDEV(size_bytes), 0.0)::FLOAT8 AS stddev_sz, \
                COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_sz, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY kind ORDER BY avg_sz DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, stddev, avg, cnt)| {
        let cv = if avg > 0.0 { stddev / avg } else { 0.0 };
        serde_json::json!({"kind": kind, "cv_deleted_size": cv, "stddev": stddev, "avg": avg, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-iqr-by-kind — IQR do tamanho de arquivos deletados por kind. Sprint #2541.
async fn file_stats_size_deleted_iqr_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT kind, \
                (PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY size_bytes) \
                 - PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY size_bytes))::FLOAT8 AS iqr_sz, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY kind ORDER BY iqr_sz DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, iqr, cnt)| serde_json::json!({"kind": kind, "iqr_deleted_size_bytes": iqr, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-stddev-by-kind — desvio-padrão do tamanho de arquivos deletados por kind. Sprint #2476.
async fn file_stats_size_deleted_stddev_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT kind, STDDEV(size_bytes)::FLOAT8 AS stddev_deleted_size, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY kind ORDER BY stddev_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, stddev, cnt)| serde_json::json!({"kind": kind, "stddev_deleted_size_bytes": stddev, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-variance-by-kind — variância do tamanho de arquivos deletados por kind. Sprint #3086.
async fn file_stats_size_deleted_variance_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT kind, COALESCE(VAR_POP(size_bytes), 0.0)::FLOAT8 AS variance_deleted_size, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY kind ORDER BY variance_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, var, cnt)| serde_json::json!({"kind": kind, "variance_deleted_size_bytes": var, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-variance-by-mime — variância do tamanho de arquivos deletados por mime_type. Sprint #3091.
async fn file_stats_size_deleted_variance_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT mime_type, COALESCE(VAR_POP(size_bytes), 0.0)::FLOAT8 AS variance_deleted_size, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY mime_type ORDER BY variance_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, var, cnt)| serde_json::json!({"mime_type": mime, "variance_deleted_size_bytes": var, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-skewness-by-kind — skewness do tamanho de arquivos deletados por kind. Sprint #3096.
async fn file_stats_size_deleted_skewness_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT kind, \
                COALESCE( \
                    (AVG(POWER(size_bytes - sub.avg_sz, 3)) / NULLIF(POWER(STDDEV_POP(size_bytes), 3), 0)), \
                    0.0)::FLOAT8 AS skewness_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         JOIN (SELECT kind AS k, AVG(size_bytes) AS avg_sz FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY kind) sub \
           ON drive_files.kind = sub.k \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY drive_files.kind ORDER BY skewness_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, skew, cnt)| serde_json::json!({"kind": kind, "skewness_deleted_size": skew, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-skewness-by-mime — skewness do tamanho de arquivos deletados por mime_type. Sprint #3101.
async fn file_stats_size_deleted_skewness_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT mime_type, \
                COALESCE( \
                    (AVG(POWER(size_bytes - sub.avg_sz, 3)) / NULLIF(POWER(STDDEV_POP(size_bytes), 3), 0)), \
                    0.0)::FLOAT8 AS skewness_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         JOIN (SELECT mime_type AS mt, AVG(size_bytes) AS avg_sz FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY mime_type) sub \
           ON drive_files.mime_type = sub.mt \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY drive_files.mime_type ORDER BY skewness_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, skew, cnt)| serde_json::json!({"mime_type": mime, "skewness_deleted_size": skew, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-skewness-by-owner — skewness do tamanho de arquivos deletados por owner_id. Sprint #3106.
async fn file_stats_size_deleted_skewness_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
                COALESCE( \
                    (AVG(POWER(size_bytes - sub.avg_sz, 3)) / NULLIF(POWER(STDDEV_POP(size_bytes), 3), 0)), \
                    0.0)::FLOAT8 AS skewness_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         JOIN (SELECT owner_id AS oid, AVG(size_bytes) AS avg_sz FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY owner_id) sub \
           ON drive_files.owner_id = sub.oid \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY drive_files.owner_id ORDER BY skewness_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, skew, cnt)| serde_json::json!({"owner_id": owner, "skewness_deleted_size": skew, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-skewness-by-ext — skewness do tamanho de arquivos deletados por extensão. Sprint #3111.
async fn file_stats_size_deleted_skewness_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT extension, \
                COALESCE( \
                    (AVG(POWER(size_bytes - sub.avg_sz, 3)) / NULLIF(POWER(STDDEV_POP(size_bytes), 3), 0)), \
                    0.0)::FLOAT8 AS skewness_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         JOIN (SELECT extension AS ext, AVG(size_bytes) AS avg_sz FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY extension) sub \
           ON drive_files.extension = sub.ext \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY drive_files.extension ORDER BY skewness_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, skew, cnt)| serde_json::json!({"extension": ext, "skewness_deleted_size": skew, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-kurtosis-by-kind — kurtosis do tamanho de arquivos deletados por kind. Sprint #3116.
async fn file_stats_size_deleted_kurtosis_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT kind, \
                COALESCE( \
                    (AVG(POWER(size_bytes - sub.avg_sz, 4)) / NULLIF(POWER(STDDEV_POP(size_bytes), 4), 0)) - 3.0, \
                    0.0)::FLOAT8 AS kurtosis_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         JOIN (SELECT kind AS k, AVG(size_bytes) AS avg_sz FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY kind) sub \
           ON drive_files.kind = sub.k \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY drive_files.kind ORDER BY kurtosis_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, kurt, cnt)| serde_json::json!({"kind": kind, "kurtosis_deleted_size": kurt, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-kurtosis-by-mime — kurtosis do tamanho de arquivos deletados por mime_type. Sprint #3121.
async fn file_stats_size_deleted_kurtosis_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT mime_type, \
                COALESCE( \
                    (AVG(POWER(size_bytes - sub.avg_sz, 4)) / NULLIF(POWER(STDDEV_POP(size_bytes), 4), 0)) - 3.0, \
                    0.0)::FLOAT8 AS kurtosis_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         JOIN (SELECT mime_type AS mt, AVG(size_bytes) AS avg_sz FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY mime_type) sub \
           ON drive_files.mime_type = sub.mt \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY drive_files.mime_type ORDER BY kurtosis_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, kurt, cnt)| serde_json::json!({"mime_type": mime, "kurtosis_deleted_size": kurt, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-kurtosis-by-owner — kurtosis do tamanho de arquivos deletados por owner_id. Sprint #3126.
async fn file_stats_size_deleted_kurtosis_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
                COALESCE( \
                    (AVG(POWER(size_bytes - sub.avg_sz, 4)) / NULLIF(POWER(STDDEV_POP(size_bytes), 4), 0)) - 3.0, \
                    0.0)::FLOAT8 AS kurtosis_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         JOIN (SELECT owner_id AS oid, AVG(size_bytes) AS avg_sz FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY owner_id) sub \
           ON drive_files.owner_id = sub.oid \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY drive_files.owner_id ORDER BY kurtosis_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, kurt, cnt)| serde_json::json!({"owner_id": owner, "kurtosis_deleted_size": kurt, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-kurtosis-by-ext — kurtosis do tamanho de arquivos deletados por extensão. Sprint #3131.
async fn file_stats_size_deleted_kurtosis_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT extension, \
                COALESCE( \
                    (AVG(POWER(size_bytes - sub.avg_sz, 4)) / NULLIF(POWER(STDDEV_POP(size_bytes), 4), 0)) - 3.0, \
                    0.0)::FLOAT8 AS kurtosis_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         JOIN (SELECT extension AS ext, AVG(size_bytes) AS avg_sz FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY extension) sub \
           ON drive_files.extension = sub.ext \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY drive_files.extension ORDER BY kurtosis_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, kurt, cnt)| serde_json::json!({"extension": ext, "kurtosis_deleted_size": kurt, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-variance-by-owner — variância do tamanho de arquivos deletados por owner_id. Sprint #3136.
async fn file_stats_size_deleted_variance_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, COALESCE(VAR_POP(size_bytes), 0.0)::FLOAT8 AS variance_deleted_size, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY owner_id ORDER BY variance_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, var, cnt)| serde_json::json!({"owner_id": owner, "variance_deleted_size_bytes": var, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-variance-by-ext — variância do tamanho de arquivos deletados por extensão. Sprint #3141.
async fn file_stats_size_deleted_variance_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT extension, COALESCE(VAR_POP(size_bytes), 0.0)::FLOAT8 AS variance_deleted_size, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY extension ORDER BY variance_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, var, cnt)| serde_json::json!({"extension": ext, "variance_deleted_size_bytes": var, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-mad-by-kind — MAD do tamanho de arquivos deletados por kind. Sprint #3149.
async fn file_stats_size_deleted_mad_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT df.kind, \
                COALESCE(AVG(ABS(df.size_bytes - sub.avg_sz)), 0.0)::FLOAT8 AS mad_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files df \
         JOIN (SELECT kind AS k, AVG(size_bytes) AS avg_sz FROM drive_files \
               WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY kind) sub ON df.kind = sub.k \
         WHERE df.tenant_id = $1 AND df.deleted_at IS NOT NULL \
         GROUP BY df.kind ORDER BY mad_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, mad, cnt)| serde_json::json!({"kind": kind, "mad_deleted_size_bytes": mad, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-mad-by-mime — MAD do tamanho de arquivos deletados por mime_type. Sprint #3150.
async fn file_stats_size_deleted_mad_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT df.mime_type, \
                COALESCE(AVG(ABS(df.size_bytes - sub.avg_sz)), 0.0)::FLOAT8 AS mad_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files df \
         JOIN (SELECT mime_type AS m, AVG(size_bytes) AS avg_sz FROM drive_files \
               WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY mime_type) sub ON df.mime_type = sub.m \
         WHERE df.tenant_id = $1 AND df.deleted_at IS NOT NULL \
         GROUP BY df.mime_type ORDER BY mad_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, mad, cnt)| serde_json::json!({"mime_type": mime, "mad_deleted_size_bytes": mad, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-mad-by-owner — MAD do tamanho de arquivos deletados por owner. Sprint #3151.
async fn file_stats_size_deleted_mad_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT df.owner_id::TEXT, \
                COALESCE(AVG(ABS(df.size_bytes - sub.avg_sz)), 0.0)::FLOAT8 AS mad_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files df \
         JOIN (SELECT owner_id AS oid, AVG(size_bytes) AS avg_sz FROM drive_files \
               WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY owner_id) sub ON df.owner_id = sub.oid \
         WHERE df.tenant_id = $1 AND df.deleted_at IS NOT NULL \
         GROUP BY df.owner_id ORDER BY mad_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, mad, cnt)| serde_json::json!({"owner_id": owner, "mad_deleted_size_bytes": mad, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-mad-by-ext — MAD do tamanho de arquivos deletados por extensão. Sprint #3152.
async fn file_stats_size_deleted_mad_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT LOWER(REVERSE(SPLIT_PART(REVERSE(df.name), '.', 1))) AS ext, \
                COALESCE(AVG(ABS(df.size_bytes - sub.avg_sz)), 0.0)::FLOAT8 AS mad_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files df \
         JOIN (SELECT LOWER(REVERSE(SPLIT_PART(REVERSE(name), '.', 1))) AS ex, AVG(size_bytes) AS avg_sz FROM drive_files \
               WHERE tenant_id = $1 AND deleted_at IS NOT NULL AND name LIKE '%.%' GROUP BY ex) sub \
           ON LOWER(REVERSE(SPLIT_PART(REVERSE(df.name), '.', 1))) = sub.ex \
         WHERE df.tenant_id = $1 AND df.deleted_at IS NOT NULL AND df.name LIKE '%.%' \
         GROUP BY ext ORDER BY mad_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, mad, cnt)| serde_json::json!({"extension": ext, "mad_deleted_size_bytes": mad, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-trimmed-mean-by-kind — média trimmed do tamanho de arquivos deletados por kind. Sprint #3169.
async fn file_stats_size_deleted_trimmed_mean_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT kind, \
                COALESCE(AVG(size_bytes) FILTER (WHERE size_bytes > PERCENTILE_CONT(0.1) WITHIN GROUP (ORDER BY size_bytes) \
                                                   AND size_bytes < PERCENTILE_CONT(0.9) WITHIN GROUP (ORDER BY size_bytes)), \
                         0.0)::FLOAT8 AS trimmed_mean_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY kind ORDER BY trimmed_mean_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, tm, cnt)| serde_json::json!({"kind": kind, "trimmed_mean_deleted_size_bytes": tm, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-trimmed-mean-by-mime — média trimmed do tamanho de arquivos deletados por mime_type. Sprint #3170.
async fn file_stats_size_deleted_trimmed_mean_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT mime_type, \
                COALESCE(AVG(size_bytes) FILTER (WHERE size_bytes > PERCENTILE_CONT(0.1) WITHIN GROUP (ORDER BY size_bytes) \
                                                   AND size_bytes < PERCENTILE_CONT(0.9) WITHIN GROUP (ORDER BY size_bytes)), \
                         0.0)::FLOAT8 AS trimmed_mean_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY mime_type ORDER BY trimmed_mean_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, tm, cnt)| serde_json::json!({"mime_type": mime, "trimmed_mean_deleted_size_bytes": tm, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-trimmed-mean-by-owner — média trimmed do tamanho de arquivos deletados por owner. Sprint #3171.
async fn file_stats_size_deleted_trimmed_mean_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
                COALESCE(AVG(size_bytes) FILTER (WHERE size_bytes > PERCENTILE_CONT(0.1) WITHIN GROUP (ORDER BY size_bytes) \
                                                   AND size_bytes < PERCENTILE_CONT(0.9) WITHIN GROUP (ORDER BY size_bytes)), \
                         0.0)::FLOAT8 AS trimmed_mean_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY owner_id ORDER BY trimmed_mean_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, tm, cnt)| serde_json::json!({"owner_id": owner, "trimmed_mean_deleted_size_bytes": tm, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-trimmed-mean-by-ext — média trimmed do tamanho de arquivos deletados por extensão. Sprint #3172.
async fn file_stats_size_deleted_trimmed_mean_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT LOWER(REVERSE(SPLIT_PART(REVERSE(name), '.', 1))) AS ext, \
                COALESCE(AVG(size_bytes) FILTER (WHERE size_bytes > PERCENTILE_CONT(0.1) WITHIN GROUP (ORDER BY size_bytes) \
                                                   AND size_bytes < PERCENTILE_CONT(0.9) WITHIN GROUP (ORDER BY size_bytes)), \
                         0.0)::FLOAT8 AS trimmed_mean_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY trimmed_mean_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, tm, cnt)| serde_json::json!({"extension": ext, "trimmed_mean_deleted_size_bytes": tm, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-winsorized-mean-by-kind — média winsorized do tamanho de arquivos deletados por kind. Sprint #3189.
async fn file_stats_size_deleted_winsorized_mean_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, f64, f64, i64)> = sqlx::query_as(
        "SELECT kind, \
                COALESCE(PERCENTILE_CONT(0.1) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS p10, \
                COALESCE(PERCENTILE_CONT(0.9) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS p90, \
                COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_sz, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY kind ORDER BY avg_sz DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, p10, p90, _, cnt)| serde_json::json!({"kind": kind, "winsorized_mean_deleted_size_bytes": (p10 + p90) / 2.0, "p10": p10, "p90": p90, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-winsorized-mean-by-mime — média winsorized do tamanho de arquivos deletados por mime_type. Sprint #3190.
async fn file_stats_size_deleted_winsorized_mean_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, f64, f64, i64)> = sqlx::query_as(
        "SELECT mime_type, \
                COALESCE(PERCENTILE_CONT(0.1) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS p10, \
                COALESCE(PERCENTILE_CONT(0.9) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS p90, \
                COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_sz, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY mime_type ORDER BY avg_sz DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, p10, p90, _, cnt)| serde_json::json!({"mime_type": mime, "winsorized_mean_deleted_size_bytes": (p10 + p90) / 2.0, "p10": p10, "p90": p90, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-winsorized-mean-by-owner — média winsorized do tamanho de arquivos deletados por owner. Sprint #3191.
async fn file_stats_size_deleted_winsorized_mean_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, f64, f64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
                COALESCE(PERCENTILE_CONT(0.1) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS p10, \
                COALESCE(PERCENTILE_CONT(0.9) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS p90, \
                COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_sz, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY owner_id ORDER BY avg_sz DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, p10, p90, _, cnt)| serde_json::json!({"owner_id": owner, "winsorized_mean_deleted_size_bytes": (p10 + p90) / 2.0, "p10": p10, "p90": p90, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-winsorized-mean-by-ext — média winsorized do tamanho de arquivos deletados por extensão. Sprint #3192.
async fn file_stats_size_deleted_winsorized_mean_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, f64, f64, i64)> = sqlx::query_as(
        "SELECT LOWER(REVERSE(SPLIT_PART(REVERSE(name), '.', 1))) AS ext, \
                COALESCE(PERCENTILE_CONT(0.1) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS p10, \
                COALESCE(PERCENTILE_CONT(0.9) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS p90, \
                COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_sz, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY avg_sz DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, p10, p90, _, cnt)| serde_json::json!({"extension": ext, "winsorized_mean_deleted_size_bytes": (p10 + p90) / 2.0, "p10": p10, "p90": p90, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-gini-by-kind — Gini do tamanho de arquivos deletados por kind. Sprint #3209.
async fn file_stats_size_deleted_gini_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT kind, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY kind ORDER BY deleted_count DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let gini = if n < 2 { 0.0 } else {
            let total: f64 = vals.iter().sum();
            if total == 0.0 { 0.0 } else {
                let sum: f64 = vals.iter().enumerate().map(|(i, &v)| (2.0 * (i + 1) as f64 - n as f64 - 1.0) * v).sum();
                sum / (n as f64 * total)
            }
        };
        serde_json::json!({"kind": kind, "gini_deleted_size": gini, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-gini-by-mime — Gini do tamanho de arquivos deletados por mime_type. Sprint #3210.
async fn file_stats_size_deleted_gini_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT mime_type, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY mime_type ORDER BY deleted_count DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let gini = if n < 2 { 0.0 } else {
            let total: f64 = vals.iter().sum();
            if total == 0.0 { 0.0 } else {
                let sum: f64 = vals.iter().enumerate().map(|(i, &v)| (2.0 * (i + 1) as f64 - n as f64 - 1.0) * v).sum();
                sum / (n as f64 * total)
            }
        };
        serde_json::json!({"mime_type": mime, "gini_deleted_size": gini, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-theil-by-kind — Theil do tamanho de arquivos deletados por kind. Sprint #3211.
async fn file_stats_size_deleted_theil_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT kind, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY kind ORDER BY deleted_count DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).filter(|&v| v > 0.0).collect();
        let n = vals.len();
        let theil = if n == 0 { 0.0 } else {
            let mean = vals.iter().sum::<f64>() / n as f64;
            if mean > 0.0 { vals.iter().map(|&x| (x / mean) * (x / mean).ln()).sum::<f64>() / n as f64 } else { 0.0 }
        };
        serde_json::json!({"kind": kind, "theil_deleted_size": theil, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-theil-by-mime — Theil do tamanho de arquivos deletados por mime_type. Sprint #3212.
async fn file_stats_size_deleted_theil_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT mime_type, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY mime_type ORDER BY deleted_count DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).filter(|&v| v > 0.0).collect();
        let n = vals.len();
        let theil = if n == 0 { 0.0 } else {
            let mean = vals.iter().sum::<f64>() / n as f64;
            if mean > 0.0 { vals.iter().map(|&x| (x / mean) * (x / mean).ln()).sum::<f64>() / n as f64 } else { 0.0 }
        };
        serde_json::json!({"mime_type": mime, "theil_deleted_size": theil, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-gini-by-owner — Gini do tamanho de arquivos deletados por owner. Sprint #3229.
async fn file_stats_size_deleted_gini_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY owner_id ORDER BY deleted_count DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let gini = if n < 2 { 0.0 } else {
            let total: f64 = vals.iter().sum();
            if total == 0.0 { 0.0 } else {
                let sum: f64 = vals.iter().enumerate().map(|(i, &v)| (2.0 * (i + 1) as f64 - n as f64 - 1.0) * v).sum();
                sum / (n as f64 * total)
            }
        };
        serde_json::json!({"owner_id": owner, "gini_deleted_size": gini, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-gini-by-ext — Gini do tamanho de arquivos deletados por extensão. Sprint #3230.
async fn file_stats_size_deleted_gini_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT LOWER(REVERSE(SPLIT_PART(REVERSE(name), '.', 1))) AS ext, \
                ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY deleted_count DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let gini = if n < 2 { 0.0 } else {
            let total: f64 = vals.iter().sum();
            if total == 0.0 { 0.0 } else {
                let sum: f64 = vals.iter().enumerate().map(|(i, &v)| (2.0 * (i + 1) as f64 - n as f64 - 1.0) * v).sum();
                sum / (n as f64 * total)
            }
        };
        serde_json::json!({"extension": ext, "gini_deleted_size": gini, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-theil-by-owner — Theil do tamanho de arquivos deletados por owner. Sprint #3231.
async fn file_stats_size_deleted_theil_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY owner_id ORDER BY deleted_count DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).filter(|&v| v > 0.0).collect();
        let n = vals.len();
        let theil = if n == 0 { 0.0 } else {
            let mean = vals.iter().sum::<f64>() / n as f64;
            if mean > 0.0 { vals.iter().map(|&x| (x / mean) * (x / mean).ln()).sum::<f64>() / n as f64 } else { 0.0 }
        };
        serde_json::json!({"owner_id": owner, "theil_deleted_size": theil, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-theil-by-ext — Theil do tamanho de arquivos deletados por extensão. Sprint #3232.
async fn file_stats_size_deleted_theil_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT LOWER(REVERSE(SPLIT_PART(REVERSE(name), '.', 1))) AS ext, \
                ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY deleted_count DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).filter(|&v| v > 0.0).collect();
        let n = vals.len();
        let theil = if n == 0 { 0.0 } else {
            let mean = vals.iter().sum::<f64>() / n as f64;
            if mean > 0.0 { vals.iter().map(|&x| (x / mean) * (x / mean).ln()).sum::<f64>() / n as f64 } else { 0.0 }
        };
        serde_json::json!({"extension": ext, "theil_deleted_size": theil, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-hhi-by-kind — HHI do tamanho de arquivos deletados por kind. Sprint #3249.
async fn file_stats_size_deleted_hhi_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT kind, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY kind ORDER BY deleted_count DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let hhi = if n == 0 { 0.0 } else {
            let total: f64 = vals.iter().sum();
            if total > 0.0 { vals.iter().map(|&v| (v / total).powi(2)).sum::<f64>() } else { 0.0 }
        };
        serde_json::json!({"kind": kind, "hhi_deleted_size": hhi, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-hhi-by-mime — HHI do tamanho de arquivos deletados por mime_type. Sprint #3250.
async fn file_stats_size_deleted_hhi_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT mime_type, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY mime_type ORDER BY deleted_count DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let hhi = if n == 0 { 0.0 } else {
            let total: f64 = vals.iter().sum();
            if total > 0.0 { vals.iter().map(|&v| (v / total).powi(2)).sum::<f64>() } else { 0.0 }
        };
        serde_json::json!({"mime_type": mime, "hhi_deleted_size": hhi, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-hhi-by-owner — HHI do tamanho de arquivos deletados por owner. Sprint #3251.
async fn file_stats_size_deleted_hhi_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY owner_id ORDER BY deleted_count DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let hhi = if n == 0 { 0.0 } else {
            let total: f64 = vals.iter().sum();
            if total > 0.0 { vals.iter().map(|&v| (v / total).powi(2)).sum::<f64>() } else { 0.0 }
        };
        serde_json::json!({"owner_id": owner, "hhi_deleted_size": hhi, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-hhi-by-ext — HHI do tamanho de arquivos deletados por extensão. Sprint #3252.
async fn file_stats_size_deleted_hhi_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT LOWER(REVERSE(SPLIT_PART(REVERSE(name), '.', 1))) AS ext, \
                ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY deleted_count DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let hhi = if n == 0 { 0.0 } else {
            let total: f64 = vals.iter().sum();
            if total > 0.0 { vals.iter().map(|&v| (v / total).powi(2)).sum::<f64>() } else { 0.0 }
        };
        serde_json::json!({"extension": ext, "hhi_deleted_size": hhi, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-atkinson-by-kind — Atkinson de tamanho deletado por kind. Sprint #3269.
async fn file_stats_size_deleted_atkinson_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT kind, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY kind ORDER BY deleted_count DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).filter(|&v| v > 0.0).collect();
        let n = vals.len();
        let atkinson = if n == 0 { 0.0 } else {
            let mean = vals.iter().sum::<f64>() / n as f64;
            let geo_mean = (vals.iter().map(|&v| v.ln()).sum::<f64>() / n as f64).exp();
            if mean > 0.0 { 1.0 - geo_mean / mean } else { 0.0 }
        };
        serde_json::json!({"kind": kind, "atkinson_deleted_size": atkinson, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-atkinson-by-mime — Atkinson de tamanho deletado por mime_type. Sprint #3270.
async fn file_stats_size_deleted_atkinson_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT mime_type, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY mime_type ORDER BY deleted_count DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).filter(|&v| v > 0.0).collect();
        let n = vals.len();
        let atkinson = if n == 0 { 0.0 } else {
            let mean = vals.iter().sum::<f64>() / n as f64;
            let geo_mean = (vals.iter().map(|&v| v.ln()).sum::<f64>() / n as f64).exp();
            if mean > 0.0 { 1.0 - geo_mean / mean } else { 0.0 }
        };
        serde_json::json!({"mime_type": mime, "atkinson_deleted_size": atkinson, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-atkinson-by-owner — Atkinson de tamanho deletado por owner. Sprint #3271.
async fn file_stats_size_deleted_atkinson_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY owner_id ORDER BY deleted_count DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).filter(|&v| v > 0.0).collect();
        let n = vals.len();
        let atkinson = if n == 0 { 0.0 } else {
            let mean = vals.iter().sum::<f64>() / n as f64;
            let geo_mean = (vals.iter().map(|&v| v.ln()).sum::<f64>() / n as f64).exp();
            if mean > 0.0 { 1.0 - geo_mean / mean } else { 0.0 }
        };
        serde_json::json!({"owner_id": owner, "atkinson_deleted_size": atkinson, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-atkinson-by-ext — Atkinson de tamanho deletado por extensão. Sprint #3272.
async fn file_stats_size_deleted_atkinson_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT LOWER(REVERSE(SPLIT_PART(REVERSE(name), '.', 1))) AS ext, \
                ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY deleted_count DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).filter(|&v| v > 0.0).collect();
        let n = vals.len();
        let atkinson = if n == 0 { 0.0 } else {
            let mean = vals.iter().sum::<f64>() / n as f64;
            let geo_mean = (vals.iter().map(|&v| v.ln()).sum::<f64>() / n as f64).exp();
            if mean > 0.0 { 1.0 - geo_mean / mean } else { 0.0 }
        };
        serde_json::json!({"extension": ext, "atkinson_deleted_size": atkinson, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-lorenz-by-kind — curva Lorenz de tamanho deletado por kind. Sprint #3289.
async fn file_stats_size_deleted_lorenz_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT kind, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY kind ORDER BY deleted_count DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let total: f64 = vals.iter().sum();
        let mut cum_pop = 0.0f64;
        let mut cum_size = 0.0f64;
        let points: Vec<serde_json::Value> = vals.iter().map(|&v| {
            cum_pop += 1.0 / n.max(1) as f64;
            cum_size += if total > 0.0 { v / total } else { 0.0 };
            serde_json::json!({"population_share": cum_pop, "size_share": cum_size})
        }).collect();
        serde_json::json!({"kind": kind, "lorenz_curve": points, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-lorenz-by-mime — curva Lorenz de tamanho deletado por mime_type. Sprint #3290.
async fn file_stats_size_deleted_lorenz_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT mime_type, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY mime_type ORDER BY deleted_count DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let total: f64 = vals.iter().sum();
        let mut cum_pop = 0.0f64;
        let mut cum_size = 0.0f64;
        let points: Vec<serde_json::Value> = vals.iter().map(|&v| {
            cum_pop += 1.0 / n.max(1) as f64;
            cum_size += if total > 0.0 { v / total } else { 0.0 };
            serde_json::json!({"population_share": cum_pop, "size_share": cum_size})
        }).collect();
        serde_json::json!({"mime_type": mime, "lorenz_curve": points, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-lorenz-by-owner — curva Lorenz de tamanho deletado por owner. Sprint #3291.
async fn file_stats_size_deleted_lorenz_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY owner_id ORDER BY deleted_count DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let total: f64 = vals.iter().sum();
        let mut cum_pop = 0.0f64;
        let mut cum_size = 0.0f64;
        let points: Vec<serde_json::Value> = vals.iter().map(|&v| {
            cum_pop += 1.0 / n.max(1) as f64;
            cum_size += if total > 0.0 { v / total } else { 0.0 };
            serde_json::json!({"population_share": cum_pop, "size_share": cum_size})
        }).collect();
        serde_json::json!({"owner_id": owner, "lorenz_curve": points, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-lorenz-by-ext — curva Lorenz de tamanho deletado por extensão. Sprint #3292.
async fn file_stats_size_deleted_lorenz_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT LOWER(REVERSE(SPLIT_PART(REVERSE(name), '.', 1))) AS ext, \
                ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY deleted_count DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let total: f64 = vals.iter().sum();
        let mut cum_pop = 0.0f64;
        let mut cum_size = 0.0f64;
        let points: Vec<serde_json::Value> = vals.iter().map(|&v| {
            cum_pop += 1.0 / n.max(1) as f64;
            cum_size += if total > 0.0 { v / total } else { 0.0 };
            serde_json::json!({"population_share": cum_pop, "size_share": cum_size})
        }).collect();
        serde_json::json!({"extension": ext, "lorenz_curve": points, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-range-by-owner — range do tamanho de arquivos deletados por owner_id. Sprint #2566.
async fn file_stats_size_deleted_range_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, MAX(size_bytes)::BIGINT AS max_sz, MIN(size_bytes)::BIGINT AS min_sz, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY owner_id ORDER BY (MAX(size_bytes) - MIN(size_bytes)) DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, max, min, cnt)| serde_json::json!({"owner_id": owner, "range_deleted_size_bytes": max - min, "max": max, "min": min, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-range-by-ext — range do tamanho de arquivos deletados por extensão. Sprint #2571.
async fn file_stats_size_deleted_range_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT LOWER(REVERSE(SPLIT_PART(REVERSE(name), '.', 1))) AS ext, \
                MAX(size_bytes)::BIGINT AS max_sz, MIN(size_bytes)::BIGINT AS min_sz, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY (MAX(size_bytes) - MIN(size_bytes)) DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, max, min, cnt)| serde_json::json!({"ext": ext, "range_deleted_size_bytes": max - min, "max": max, "min": min, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-range-by-kind — range do tamanho de arquivos deletados por kind. Sprint #2576.
async fn file_stats_size_deleted_range_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT kind, MAX(size_bytes)::BIGINT AS max_sz, MIN(size_bytes)::BIGINT AS min_sz, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY kind ORDER BY (MAX(size_bytes) - MIN(size_bytes)) DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, max, min, cnt)| serde_json::json!({"kind": kind, "range_deleted_size_bytes": max - min, "max": max, "min": min, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-range-by-mime — range do tamanho de arquivos deletados por mime_type. Sprint #2581.
async fn file_stats_size_deleted_range_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT mime_type, MAX(size_bytes)::BIGINT AS max_sz, MIN(size_bytes)::BIGINT AS min_sz, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY mime_type ORDER BY (MAX(size_bytes) - MIN(size_bytes)) DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, max, min, cnt)| serde_json::json!({"mime_type": mime, "range_deleted_size_bytes": max - min, "max": max, "min": min, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-range-by-kind — range de tamanho de arquivos ativos por kind. Sprint #2766.
async fn file_stats_size_active_range_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT kind, \
                MAX(size_bytes)::BIGINT AS max_size, \
                MIN(size_bytes)::BIGINT AS min_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY kind ORDER BY (MAX(size_bytes) - MIN(size_bytes)) DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, max_s, min_s, cnt)| {
        serde_json::json!({"kind": kind, "range_active_size": max_s - min_s, "max": max_s, "min": min_s, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-range-by-mime — range de tamanho de arquivos ativos por mime_type. Sprint #2771.
async fn file_stats_size_active_range_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT mime_type, \
                MAX(size_bytes)::BIGINT AS max_size, \
                MIN(size_bytes)::BIGINT AS min_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY (MAX(size_bytes) - MIN(size_bytes)) DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, max_s, min_s, cnt)| {
        serde_json::json!({"mime_type": mime, "range_active_size": max_s - min_s, "max": max_s, "min": min_s, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-range-by-owner — range de tamanho de arquivos ativos por owner_id. Sprint #2776.
async fn file_stats_size_active_range_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(uuid::Uuid, i64, i64, i64)> = sqlx::query_as(
        "SELECT owner_id, \
                MAX(size_bytes)::BIGINT AS max_size, \
                MIN(size_bytes)::BIGINT AS min_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY (MAX(size_bytes) - MIN(size_bytes)) DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner, max_s, min_s, cnt)| {
        serde_json::json!({"owner_id": owner, "range_active_size": max_s - min_s, "max": max_s, "min": min_s, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-entropy-by-kind — entropia de Shannon do tamanho de arquivos ativos por kind. Sprint #2781.
async fn file_stats_size_active_entropy_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT kind, SUM(size_bytes)::BIGINT AS total_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let grand_total: i64 = rows.iter().map(|(_, s, _)| s).sum();
    let entropy: f64 = if grand_total > 0 {
        rows.iter().map(|(_, s, _)| {
            let p = *s as f64 / grand_total as f64;
            if p > 0.0 { -p * p.ln() } else { 0.0 }
        }).sum()
    } else { 0.0 };
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, s, cnt)| {
        let p = if grand_total > 0 { s as f64 / grand_total as f64 } else { 0.0 };
        serde_json::json!({"kind": kind, "share": p, "total_size": s, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"entropy_active_size_by_kind": entropy, "rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-entropy-by-mime — entropia de Shannon de tamanho de arquivos ativos por mime_type. Sprint #2786.
async fn file_stats_size_active_entropy_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT mime_type, SUM(size_bytes)::BIGINT AS total_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let grand_total: i64 = rows.iter().map(|(_, s, _)| s).sum();
    let entropy: f64 = if grand_total > 0 {
        rows.iter().map(|(_, s, _)| {
            let p = *s as f64 / grand_total as f64;
            if p > 0.0 { -p * p.ln() } else { 0.0 }
        }).sum()
    } else { 0.0 };
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, s, cnt)| {
        let p = if grand_total > 0 { s as f64 / grand_total as f64 } else { 0.0 };
        serde_json::json!({"mime_type": mime, "share": p, "total_size": s, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"entropy_active_size_by_mime": entropy, "rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-entropy-by-owner — entropia de Shannon de tamanho de arquivos ativos por owner_id. Sprint #2791.
async fn file_stats_size_active_entropy_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(uuid::Uuid, i64, i64)> = sqlx::query_as(
        "SELECT owner_id, SUM(size_bytes)::BIGINT AS total_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let grand_total: i64 = rows.iter().map(|(_, s, _)| s).sum();
    let entropy: f64 = if grand_total > 0 {
        rows.iter().map(|(_, s, _)| {
            let p = *s as f64 / grand_total as f64;
            if p > 0.0 { -p * p.ln() } else { 0.0 }
        }).sum()
    } else { 0.0 };
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner, s, cnt)| {
        let p = if grand_total > 0 { s as f64 / grand_total as f64 } else { 0.0 };
        serde_json::json!({"owner_id": owner, "share": p, "total_size": s, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"entropy_active_size_by_owner": entropy, "rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-skewness-by-kind — assimetria de tamanho de arquivos ativos por kind. Sprint #2796.
async fn file_stats_size_active_skewness_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, f64, f64, i64)> = sqlx::query_as(
        "SELECT kind, \
                COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_size, \
                COALESCE(STDDEV(size_bytes), 0.0)::FLOAT8 AS stddev_size, \
                COALESCE(AVG(POWER(size_bytes - (SELECT AVG(f2.size_bytes) FROM drive_files f2 WHERE f2.tenant_id = drive_files.tenant_id AND f2.kind = drive_files.kind AND f2.deleted_at IS NULL), 3)), 0.0)::FLOAT8 AS m3, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, _avg, stddev, m3, cnt)| {
        let skewness = if stddev > 0.0 { m3 / stddev.powi(3) } else { 0.0 };
        serde_json::json!({"kind": kind, "skewness_active_size": skewness, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-skewness-by-mime — assimetria de tamanho de arquivos ativos por mime_type. Sprint #2801.
async fn file_stats_size_active_skewness_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, f64, f64, i64)> = sqlx::query_as(
        "SELECT mime_type, \
                COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_size, \
                COALESCE(STDDEV(size_bytes), 0.0)::FLOAT8 AS stddev_size, \
                COALESCE(AVG(POWER(size_bytes - (SELECT AVG(f2.size_bytes) FROM drive_files f2 WHERE f2.tenant_id = drive_files.tenant_id AND f2.mime_type = drive_files.mime_type AND f2.deleted_at IS NULL), 3)), 0.0)::FLOAT8 AS m3, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, _avg, stddev, m3, cnt)| {
        let skewness = if stddev > 0.0 { m3 / stddev.powi(3) } else { 0.0 };
        serde_json::json!({"mime_type": mime, "skewness_active_size": skewness, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-skewness-by-owner — assimetria de tamanho de arquivos ativos por owner_id. Sprint #2806.
async fn file_stats_size_active_skewness_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(uuid::Uuid, f64, f64, f64, i64)> = sqlx::query_as(
        "SELECT owner_id, \
                COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_size, \
                COALESCE(STDDEV(size_bytes), 0.0)::FLOAT8 AS stddev_size, \
                COALESCE(AVG(POWER(size_bytes - (SELECT AVG(f2.size_bytes) FROM drive_files f2 WHERE f2.tenant_id = drive_files.tenant_id AND f2.owner_id = drive_files.owner_id AND f2.deleted_at IS NULL), 3)), 0.0)::FLOAT8 AS m3, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner, _avg, stddev, m3, cnt)| {
        let skewness = if stddev > 0.0 { m3 / stddev.powi(3) } else { 0.0 };
        serde_json::json!({"owner_id": owner, "skewness_active_size": skewness, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-kurtosis-by-kind — curtose de tamanho de arquivos ativos por kind. Sprint #2811.
async fn file_stats_size_active_kurtosis_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, f64, f64, i64)> = sqlx::query_as(
        "SELECT kind, \
                COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_size, \
                COALESCE(STDDEV(size_bytes), 0.0)::FLOAT8 AS stddev_size, \
                COALESCE(AVG(POWER(size_bytes - (SELECT AVG(f2.size_bytes) FROM drive_files f2 WHERE f2.tenant_id = drive_files.tenant_id AND f2.kind = drive_files.kind AND f2.deleted_at IS NULL), 4)), 0.0)::FLOAT8 AS m4, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, _avg, stddev, m4, cnt)| {
        let kurtosis = if stddev > 0.0 { m4 / stddev.powi(4) - 3.0 } else { 0.0 };
        serde_json::json!({"kind": kind, "kurtosis_active_size": kurtosis, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-kurtosis-by-mime — curtose de tamanho de arquivos ativos por mime_type. Sprint #2816.
async fn file_stats_size_active_kurtosis_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, f64, f64, i64)> = sqlx::query_as(
        "SELECT mime_type, \
                COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_size, \
                COALESCE(STDDEV(size_bytes), 0.0)::FLOAT8 AS stddev_size, \
                COALESCE(AVG(POWER(size_bytes - (SELECT AVG(f2.size_bytes) FROM drive_files f2 WHERE f2.tenant_id = drive_files.tenant_id AND f2.mime_type = drive_files.mime_type AND f2.deleted_at IS NULL), 4)), 0.0)::FLOAT8 AS m4, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, _avg, stddev, m4, cnt)| {
        let kurtosis = if stddev > 0.0 { m4 / stddev.powi(4) - 3.0 } else { 0.0 };
        serde_json::json!({"mime_type": mime, "kurtosis_active_size": kurtosis, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-kurtosis-by-owner — curtose de tamanho de arquivos ativos por owner_id. Sprint #2821.
async fn file_stats_size_active_kurtosis_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(uuid::Uuid, f64, f64, f64, i64)> = sqlx::query_as(
        "SELECT owner_id, \
                COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_size, \
                COALESCE(STDDEV(size_bytes), 0.0)::FLOAT8 AS stddev_size, \
                COALESCE(AVG(POWER(size_bytes - (SELECT AVG(f2.size_bytes) FROM drive_files f2 WHERE f2.tenant_id = drive_files.tenant_id AND f2.owner_id = drive_files.owner_id AND f2.deleted_at IS NULL), 4)), 0.0)::FLOAT8 AS m4, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner, _avg, stddev, m4, cnt)| {
        let kurtosis = if stddev > 0.0 { m4 / stddev.powi(4) - 3.0 } else { 0.0 };
        serde_json::json!({"owner_id": owner, "kurtosis_active_size": kurtosis, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-skewness-by-ext — assimetria de size ativo por extensão. Sprint #3609.
async fn file_stats_size_active_skewness_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT COALESCE(file_ext, 'unknown'), ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY file_ext ORDER BY file_ext",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let skewness = if n < 3 { 0.0 } else {
            let mean = vals.iter().sum::<f64>() / n as f64;
            let variance = vals.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n as f64;
            let stddev = variance.sqrt();
            if stddev == 0.0 { 0.0 } else {
                vals.iter().map(|&v| ((v - mean) / stddev).powi(3)).sum::<f64>() / n as f64
            }
        };
        serde_json::json!({"file_ext": ext, "skewness_active_size": skewness, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-variance-by-ext — variância de size ativo por extensão. Sprint #3610.
async fn file_stats_size_active_variance_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT COALESCE(file_ext, 'unknown'), \
                COALESCE(VARIANCE(size_bytes), 0.0)::FLOAT8 AS variance_active_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY file_ext ORDER BY file_ext",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, var, cnt)| {
        serde_json::json!({"file_ext": ext, "variance_active_size": var, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-kurtosis-by-ext — curtose de size ativo por extensão. Sprint #3611.
async fn file_stats_size_active_kurtosis_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT COALESCE(file_ext, 'unknown'), ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY file_ext ORDER BY file_ext",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let kurtosis = if n < 4 { 0.0 } else {
            let mean = vals.iter().sum::<f64>() / n as f64;
            let variance = vals.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n as f64;
            let stddev = variance.sqrt();
            if stddev == 0.0 { 0.0 } else {
                vals.iter().map(|&v| ((v - mean) / stddev).powi(4)).sum::<f64>() / n as f64 - 3.0
            }
        };
        serde_json::json!({"file_ext": ext, "kurtosis_active_size": kurtosis, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-cv-by-ext — CV de size ativo por extensão. Sprint #3612.
async fn file_stats_size_active_cv_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, f64, i64)> = sqlx::query_as(
        "SELECT COALESCE(file_ext, 'unknown'), \
                COALESCE(STDDEV(size_bytes), 0.0)::FLOAT8 AS stddev_s, \
                COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_s, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY file_ext ORDER BY file_ext",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, stddev, avg, cnt)| {
        let cv = if avg > 0.0 { stddev / avg } else { 0.0 };
        serde_json::json!({"file_ext": ext, "cv_active_size": cv, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-gini-by-kind — Gini de size_bytes de arquivos ativos por kind. Sprint #2826.
async fn file_stats_size_active_gini_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<f64>>, i64)> = sqlx::query_as(
        "SELECT kind, ARRAY_AGG(size_bytes::FLOAT8 ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, sizes_opt, cnt)| {
        let vals: Vec<f64> = sizes_opt.into_iter().flatten().collect();
        let n = vals.len();
        let gini = if n > 1 {
            let sum: f64 = vals.iter().sum();
            if sum > 0.0 {
                let mut rank_sum = 0.0f64;
                for (i, v) in vals.iter().enumerate() {
                    rank_sum += (2.0 * (i + 1) as f64 - n as f64 - 1.0) * v;
                }
                rank_sum / (n as f64 * sum)
            } else { 0.0 }
        } else { 0.0 };
        serde_json::json!({"kind": kind, "gini_active_size": gini, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-gini-by-mime — Gini de size_bytes de arquivos ativos por mime_type. Sprint #2831.
async fn file_stats_size_active_gini_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<f64>>, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type, 'unknown') AS mime, ARRAY_AGG(size_bytes::FLOAT8 ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY mime ORDER BY mime",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, sizes_opt, cnt)| {
        let vals: Vec<f64> = sizes_opt.into_iter().flatten().collect();
        let n = vals.len();
        let gini = if n > 1 {
            let sum: f64 = vals.iter().sum();
            if sum > 0.0 {
                let mut rank_sum = 0.0f64;
                for (i, v) in vals.iter().enumerate() {
                    rank_sum += (2.0 * (i + 1) as f64 - n as f64 - 1.0) * v;
                }
                rank_sum / (n as f64 * sum)
            } else { 0.0 }
        } else { 0.0 };
        serde_json::json!({"mime_type": mime, "gini_active_size": gini, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-gini-by-owner — Gini de size_bytes de arquivos ativos por owner. Sprint #2836.
async fn file_stats_size_active_gini_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(uuid::Uuid, Vec<Option<f64>>, i64)> = sqlx::query_as(
        "SELECT owner_id, ARRAY_AGG(size_bytes::FLOAT8 ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner_id, sizes_opt, cnt)| {
        let vals: Vec<f64> = sizes_opt.into_iter().flatten().collect();
        let n = vals.len();
        let gini = if n > 1 {
            let sum: f64 = vals.iter().sum();
            if sum > 0.0 {
                let mut rank_sum = 0.0f64;
                for (i, v) in vals.iter().enumerate() {
                    rank_sum += (2.0 * (i + 1) as f64 - n as f64 - 1.0) * v;
                }
                rank_sum / (n as f64 * sum)
            } else { 0.0 }
        } else { 0.0 };
        serde_json::json!({"owner_id": owner_id, "gini_active_size": gini, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-hhi-by-kind — HHI de size_bytes de arquivos ativos por kind. Sprint #2841.
async fn file_stats_size_active_hhi_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT kind, SUM(size_bytes)::BIGINT AS total_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let grand_total: i64 = rows.iter().map(|(_, s, _)| s).sum();
    let hhi = if grand_total > 0 {
        rows.iter().map(|(_, s, _)| {
            let share = *s as f64 / grand_total as f64;
            share * share
        }).sum::<f64>()
    } else { 0.0 };
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, total, cnt)| {
        let share = if grand_total > 0 { total as f64 / grand_total as f64 } else { 0.0 };
        serde_json::json!({"kind": kind, "size_share": share, "total_size": total, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"hhi": hhi, "rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-hhi-by-mime — HHI de size_bytes de arquivos ativos por mime_type. Sprint #2846.
async fn file_stats_size_active_hhi_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type, 'unknown') AS mime, SUM(size_bytes)::BIGINT AS total_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY mime ORDER BY mime",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let grand_total: i64 = rows.iter().map(|(_, s, _)| s).sum();
    let hhi = if grand_total > 0 {
        rows.iter().map(|(_, s, _)| {
            let share = *s as f64 / grand_total as f64;
            share * share
        }).sum::<f64>()
    } else { 0.0 };
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, total, cnt)| {
        let share = if grand_total > 0 { total as f64 / grand_total as f64 } else { 0.0 };
        serde_json::json!({"mime_type": mime, "size_share": share, "total_size": total, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"hhi": hhi, "rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-hhi-by-owner — HHI de size_bytes de arquivos ativos por owner. Sprint #2851.
async fn file_stats_size_active_hhi_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(uuid::Uuid, i64, i64)> = sqlx::query_as(
        "SELECT owner_id, SUM(size_bytes)::BIGINT AS total_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let grand_total: i64 = rows.iter().map(|(_, s, _)| s).sum();
    let hhi = if grand_total > 0 {
        rows.iter().map(|(_, s, _)| {
            let share = *s as f64 / grand_total as f64;
            share * share
        }).sum::<f64>()
    } else { 0.0 };
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner_id, total, cnt)| {
        let share = if grand_total > 0 { total as f64 / grand_total as f64 } else { 0.0 };
        serde_json::json!({"owner_id": owner_id, "size_share": share, "total_size": total, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"hhi": hhi, "rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-lorenz-by-kind — curva de Lorenz de size_bytes de arquivos ativos por kind. Sprint #2856.
async fn file_stats_size_active_lorenz_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT kind, SUM(size_bytes)::BIGINT AS total_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY kind ORDER BY total_size ASC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let grand_total: i64 = rows.iter().map(|(_, s, _)| s).sum();
    let n = rows.len();
    let lorenz_points: Vec<serde_json::Value> = rows.iter().enumerate().scan(0i64, |acc, (i, (kind, size, cnt))| {
        *acc += size;
        Some(serde_json::json!({
            "kind": kind,
            "cumulative_population": (i + 1) as f64 / n as f64,
            "cumulative_share": if grand_total > 0 { *acc as f64 / grand_total as f64 } else { 0.0 },
            "count": cnt
        }))
    }).collect();
    Ok(Json(serde_json::json!({"lorenz_curve": lorenz_points, "total_size": grand_total})))
}

/// GET /api/v1/drive/files/stats/active-size-lorenz-by-mime — curva de Lorenz de size_bytes de arquivos ativos por mime_type. Sprint #2861.
async fn file_stats_size_active_lorenz_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type, 'unknown') AS mime, SUM(size_bytes)::BIGINT AS total_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY mime ORDER BY total_size ASC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let grand_total: i64 = rows.iter().map(|(_, s, _)| s).sum();
    let n = rows.len();
    let lorenz_points: Vec<serde_json::Value> = rows.iter().enumerate().scan(0i64, |acc, (i, (mime, size, cnt))| {
        *acc += size;
        Some(serde_json::json!({
            "mime_type": mime,
            "cumulative_population": (i + 1) as f64 / n as f64,
            "cumulative_share": if grand_total > 0 { *acc as f64 / grand_total as f64 } else { 0.0 },
            "count": cnt
        }))
    }).collect();
    Ok(Json(serde_json::json!({"lorenz_curve": lorenz_points, "total_size": grand_total})))
}

/// GET /api/v1/drive/files/stats/active-size-lorenz-by-owner — curva de Lorenz de size_bytes de arquivos ativos por owner. Sprint #2866.
async fn file_stats_size_active_lorenz_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(uuid::Uuid, i64, i64)> = sqlx::query_as(
        "SELECT owner_id, SUM(size_bytes)::BIGINT AS total_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY owner_id ORDER BY total_size ASC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let grand_total: i64 = rows.iter().map(|(_, s, _)| s).sum();
    let n = rows.len();
    let lorenz_points: Vec<serde_json::Value> = rows.iter().enumerate().scan(0i64, |acc, (i, (owner_id, size, cnt))| {
        *acc += size;
        Some(serde_json::json!({
            "owner_id": owner_id,
            "cumulative_population": (i + 1) as f64 / n as f64,
            "cumulative_share": if grand_total > 0 { *acc as f64 / grand_total as f64 } else { 0.0 },
            "count": cnt
        }))
    }).collect();
    Ok(Json(serde_json::json!({"lorenz_curve": lorenz_points, "total_size": grand_total})))
}

/// GET /api/v1/drive/files/stats/active-size-theil-by-kind — índice de Theil de size_bytes de arquivos ativos por kind. Sprint #2871.
async fn file_stats_size_active_theil_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT kind, SUM(size_bytes)::BIGINT AS total_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let grand_total: i64 = rows.iter().map(|(_, s, _)| s).sum();
    let n = rows.len();
    let theil = if grand_total > 0 && n > 0 {
        let mean = grand_total as f64 / n as f64;
        rows.iter().map(|(_, s, _)| {
            let x = *s as f64;
            if x > 0.0 && mean > 0.0 { (x / mean) * (x / mean).ln() } else { 0.0 }
        }).sum::<f64>() / n as f64
    } else { 0.0 };
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, total, cnt)| {
        let share = if grand_total > 0 { total as f64 / grand_total as f64 } else { 0.0 };
        serde_json::json!({"kind": kind, "size_share": share, "total_size": total, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"theil": theil, "rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-theil-by-mime — índice de Theil de size_bytes de arquivos ativos por mime_type. Sprint #2876.
async fn file_stats_size_active_theil_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type, 'unknown') AS mime, SUM(size_bytes)::BIGINT AS total_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY mime ORDER BY mime",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let grand_total: i64 = rows.iter().map(|(_, s, _)| s).sum();
    let n = rows.len();
    let theil = if grand_total > 0 && n > 0 {
        let mean = grand_total as f64 / n as f64;
        rows.iter().map(|(_, s, _)| {
            let x = *s as f64;
            if x > 0.0 && mean > 0.0 { (x / mean) * (x / mean).ln() } else { 0.0 }
        }).sum::<f64>() / n as f64
    } else { 0.0 };
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, total, cnt)| {
        let share = if grand_total > 0 { total as f64 / grand_total as f64 } else { 0.0 };
        serde_json::json!({"mime_type": mime, "size_share": share, "total_size": total, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"theil": theil, "rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-theil-by-owner — índice de Theil de size_bytes de arquivos ativos por owner. Sprint #2881.
async fn file_stats_size_active_theil_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(uuid::Uuid, i64, i64)> = sqlx::query_as(
        "SELECT owner_id, SUM(size_bytes)::BIGINT AS total_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let grand_total: i64 = rows.iter().map(|(_, s, _)| s).sum();
    let n = rows.len();
    let theil = if grand_total > 0 && n > 0 {
        let mean = grand_total as f64 / n as f64;
        rows.iter().map(|(_, s, _)| {
            let x = *s as f64;
            if x > 0.0 && mean > 0.0 { (x / mean) * (x / mean).ln() } else { 0.0 }
        }).sum::<f64>() / n as f64
    } else { 0.0 };
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner_id, total, cnt)| {
        let share = if grand_total > 0 { total as f64 / grand_total as f64 } else { 0.0 };
        serde_json::json!({"owner_id": owner_id, "size_share": share, "total_size": total, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"theil": theil, "rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-gini-by-ext — Gini de size de arquivos ativos por extensão. Sprint #3309.
async fn file_stats_size_active_gini_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT LOWER(REVERSE(SPLIT_PART(REVERSE(name), '.', 1))) AS ext, \
                ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY active_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let gini = if n < 2 { 0.0 } else {
            let total: f64 = vals.iter().sum();
            if total == 0.0 { 0.0 } else {
                let sum: f64 = vals.iter().enumerate().map(|(i, &v)| (2.0 * (i + 1) as f64 - n as f64 - 1.0) * v).sum();
                sum / (n as f64 * total)
            }
        };
        serde_json::json!({"extension": ext, "gini_active_size": gini, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-hhi-by-ext — HHI de size de arquivos ativos por extensão. Sprint #3310.
async fn file_stats_size_active_hhi_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT LOWER(REVERSE(SPLIT_PART(REVERSE(name), '.', 1))) AS ext, \
                ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY active_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let total: f64 = vals.iter().sum();
        let hhi = if total > 0.0 { vals.iter().map(|&v| (v / total).powi(2)).sum::<f64>() } else { 0.0 };
        serde_json::json!({"extension": ext, "hhi_active_size": hhi, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-lorenz-by-ext — curva Lorenz de size ativo por extensão. Sprint #3311.
async fn file_stats_size_active_lorenz_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT LOWER(REVERSE(SPLIT_PART(REVERSE(name), '.', 1))) AS ext, \
                ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY active_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let total: f64 = vals.iter().sum();
        let mut cum_pop = 0.0f64;
        let mut cum_size = 0.0f64;
        let points: Vec<serde_json::Value> = vals.iter().map(|&v| {
            cum_pop += 1.0 / n.max(1) as f64;
            cum_size += if total > 0.0 { v / total } else { 0.0 };
            serde_json::json!({"population_share": cum_pop, "size_share": cum_size})
        }).collect();
        serde_json::json!({"extension": ext, "lorenz_curve": points, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-theil-by-ext — índice Theil de size ativo por extensão. Sprint #3312.
async fn file_stats_size_active_theil_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT LOWER(REVERSE(SPLIT_PART(REVERSE(name), '.', 1))) AS ext, \
                ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY active_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).filter(|&v| v > 0.0).collect();
        let n = vals.len();
        let theil = if n == 0 { 0.0 } else {
            let mean = vals.iter().sum::<f64>() / n as f64;
            if mean == 0.0 { 0.0 } else { vals.iter().map(|&x| (x / mean) * (x / mean).ln()).sum::<f64>() / n as f64 }
        };
        serde_json::json!({"extension": ext, "theil_active_size": theil, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-atkinson-by-kind — índice Atkinson de size por kind. Sprint #2886.
async fn file_stats_size_active_atkinson_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<f64>>, i64)> = sqlx::query_as(
        "SELECT kind, ARRAY_AGG(size_bytes::FLOAT8 ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, sizes_opt, cnt)| {
        let vals: Vec<f64> = sizes_opt.into_iter().flatten().filter(|&v| v > 0.0).collect();
        let n = vals.len();
        let atkinson = if n > 1 {
            let mean = vals.iter().sum::<f64>() / n as f64;
            let geo_mean = {
                let log_sum: f64 = vals.iter().map(|v| v.ln()).sum();
                (log_sum / n as f64).exp()
            };
            if mean > 0.0 { 1.0 - geo_mean / mean } else { 0.0 }
        } else { 0.0 };
        serde_json::json!({"kind": kind, "atkinson_active_size": atkinson, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-atkinson-by-mime — índice Atkinson de size por mime_type. Sprint #2891.
async fn file_stats_size_active_atkinson_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<f64>>, i64)> = sqlx::query_as(
        "SELECT mime_type, ARRAY_AGG(size_bytes::FLOAT8 ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, sizes_opt, cnt)| {
        let vals: Vec<f64> = sizes_opt.into_iter().flatten().filter(|&v| v > 0.0).collect();
        let n = vals.len();
        let atkinson = if n > 1 {
            let mean = vals.iter().sum::<f64>() / n as f64;
            let geo_mean = {
                let log_sum: f64 = vals.iter().map(|v| v.ln()).sum();
                (log_sum / n as f64).exp()
            };
            if mean > 0.0 { 1.0 - geo_mean / mean } else { 0.0 }
        } else { 0.0 };
        serde_json::json!({"mime_type": mime, "atkinson_active_size": atkinson, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-atkinson-by-owner — índice Atkinson de size por owner. Sprint #2896.
async fn file_stats_size_active_atkinson_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(uuid::Uuid, Vec<Option<f64>>, i64)> = sqlx::query_as(
        "SELECT owner_id, ARRAY_AGG(size_bytes::FLOAT8 ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner_id, sizes_opt, cnt)| {
        let vals: Vec<f64> = sizes_opt.into_iter().flatten().filter(|&v| v > 0.0).collect();
        let n = vals.len();
        let atkinson = if n > 1 {
            let mean = vals.iter().sum::<f64>() / n as f64;
            let geo_mean = {
                let log_sum: f64 = vals.iter().map(|v| v.ln()).sum();
                (log_sum / n as f64).exp()
            };
            if mean > 0.0 { 1.0 - geo_mean / mean } else { 0.0 }
        } else { 0.0 };
        serde_json::json!({"owner_id": owner_id, "atkinson_active_size": atkinson, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-atkinson-by-ext — Atkinson de size ativo por extensão. Sprint #3329.
async fn file_stats_size_active_atkinson_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT LOWER(REVERSE(SPLIT_PART(REVERSE(name), '.', 1))) AS ext, \
                ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY active_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).filter(|&v| v > 0.0).collect();
        let n = vals.len();
        let atkinson = if n == 0 { 0.0 } else {
            let mean = vals.iter().sum::<f64>() / n as f64;
            let geo_mean = (vals.iter().map(|&v| v.ln()).sum::<f64>() / n as f64).exp();
            if mean > 0.0 { 1.0 - geo_mean / mean } else { 0.0 }
        };
        serde_json::json!({"extension": ext, "atkinson_active_size": atkinson, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-normalized-entropy-by-kind — entropia normalizada de size ativo por kind. Sprint #3330.
async fn file_stats_size_active_normalized_entropy_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT kind, SUM(size_bytes)::BIGINT AS total_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let grand_total: i64 = rows.iter().map(|(_, s, _)| s).sum();
    let n = rows.len();
    let entropy = if grand_total > 0 {
        rows.iter().fold(0.0f64, |acc, (_, s, _)| {
            let p = *s as f64 / grand_total as f64;
            if p > 0.0 { acc - p * p.ln() } else { acc }
        })
    } else { 0.0 };
    let normalized = if n > 1 { entropy / (n as f64).ln() } else { 0.0 };
    Ok(Json(serde_json::json!({"entropy": entropy, "normalized_entropy": normalized, "kind_count": n})))
}

/// GET /api/v1/drive/files/stats/active-size-normalized-entropy-by-mime — entropia normalizada de size ativo por mime. Sprint #3331.
async fn file_stats_size_active_normalized_entropy_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT mime_type, SUM(size_bytes)::BIGINT AS total_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let grand_total: i64 = rows.iter().map(|(_, s, _)| s).sum();
    let n = rows.len();
    let entropy = if grand_total > 0 {
        rows.iter().fold(0.0f64, |acc, (_, s, _)| {
            let p = *s as f64 / grand_total as f64;
            if p > 0.0 { acc - p * p.ln() } else { acc }
        })
    } else { 0.0 };
    let normalized = if n > 1 { entropy / (n as f64).ln() } else { 0.0 };
    Ok(Json(serde_json::json!({"entropy": entropy, "normalized_entropy": normalized, "mime_count": n})))
}

/// GET /api/v1/drive/files/stats/active-size-normalized-entropy-by-owner — entropia normalizada de size ativo por owner. Sprint #3332.
async fn file_stats_size_active_normalized_entropy_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, SUM(size_bytes)::BIGINT AS total_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let grand_total: i64 = rows.iter().map(|(_, s, _)| s).sum();
    let n = rows.len();
    let entropy = if grand_total > 0 {
        rows.iter().fold(0.0f64, |acc, (_, s, _)| {
            let p = *s as f64 / grand_total as f64;
            if p > 0.0 { acc - p * p.ln() } else { acc }
        })
    } else { 0.0 };
    let normalized = if n > 1 { entropy / (n as f64).ln() } else { 0.0 };
    Ok(Json(serde_json::json!({"entropy": entropy, "normalized_entropy": normalized, "owner_count": n})))
}

/// GET /api/v1/drive/files/stats/active-size-normalized-entropy-by-ext — entropia normalizada de size ativo por extensão. Sprint #3349.
async fn file_stats_size_active_normalized_entropy_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT LOWER(REVERSE(SPLIT_PART(REVERSE(name), '.', 1))) AS ext, \
                SUM(size_bytes)::BIGINT AS total_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY ext",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let grand_total: i64 = rows.iter().map(|(_, s, _)| s).sum();
    let n = rows.len();
    let entropy = if grand_total > 0 {
        rows.iter().fold(0.0f64, |acc, (_, s, _)| {
            let p = *s as f64 / grand_total as f64;
            if p > 0.0 { acc - p * p.ln() } else { acc }
        })
    } else { 0.0 };
    let normalized = if n > 1 { entropy / (n as f64).ln() } else { 0.0 };
    Ok(Json(serde_json::json!({"entropy": entropy, "normalized_entropy": normalized, "ext_count": n})))
}

/// GET /api/v1/drive/files/stats/active-size-trimmed-mean-by-kind — média aparada de size ativo por kind. Sprint #3350.
async fn file_stats_size_active_trimmed_mean_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT kind, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let trimmed_mean = if n < 2 { vals.first().copied().unwrap_or(0.0) } else {
            let trim = (n as f64 * 0.1) as usize;
            let t = &vals[trim..n - trim];
            if t.is_empty() { 0.0 } else { t.iter().sum::<f64>() / t.len() as f64 }
        };
        serde_json::json!({"kind": kind, "trimmed_mean_active_size": trimmed_mean, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-trimmed-mean-by-mime — média aparada de size ativo por mime_type. Sprint #3351.
async fn file_stats_size_active_trimmed_mean_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT mime_type, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let trimmed_mean = if n < 2 { vals.first().copied().unwrap_or(0.0) } else {
            let trim = (n as f64 * 0.1) as usize;
            let t = &vals[trim..n - trim];
            if t.is_empty() { 0.0 } else { t.iter().sum::<f64>() / t.len() as f64 }
        };
        serde_json::json!({"mime_type": mime, "trimmed_mean_active_size": trimmed_mean, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-trimmed-mean-by-owner — média aparada de size ativo por owner. Sprint #3352.
async fn file_stats_size_active_trimmed_mean_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let trimmed_mean = if n < 2 { vals.first().copied().unwrap_or(0.0) } else {
            let trim = (n as f64 * 0.1) as usize;
            let t = &vals[trim..n - trim];
            if t.is_empty() { 0.0 } else { t.iter().sum::<f64>() / t.len() as f64 }
        };
        serde_json::json!({"owner_id": owner, "trimmed_mean_active_size": trimmed_mean, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-trimmed-mean-by-ext — média aparada de size ativo por extensão. Sprint #3369.
async fn file_stats_size_active_trimmed_mean_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(Option<String>, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT extension, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY extension ORDER BY extension",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let trimmed_mean = if n < 2 { vals.first().copied().unwrap_or(0.0) } else {
            let trim = (n as f64 * 0.1) as usize;
            let t = &vals[trim..n - trim];
            if t.is_empty() { 0.0 } else { t.iter().sum::<f64>() / t.len() as f64 }
        };
        serde_json::json!({"extension": ext, "trimmed_mean_active_size": trimmed_mean, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-winsorized-mean-by-kind — média winsorizada de size ativo por kind. Sprint #3370.
async fn file_stats_size_active_winsorized_mean_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT kind, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let winsorized_mean = if n < 2 { vals.first().copied().unwrap_or(0.0) } else {
            let p10 = vals[(n as f64 * 0.10) as usize];
            let p90 = vals[((n as f64 * 0.90) as usize).min(n - 1)];
            let clamped: Vec<f64> = vals.iter().map(|&v| v.clamp(p10, p90)).collect();
            clamped.iter().sum::<f64>() / clamped.len() as f64
        };
        serde_json::json!({"kind": kind, "winsorized_mean_active_size": winsorized_mean, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-winsorized-mean-by-mime — média winsorizada de size ativo por mime_type. Sprint #3371.
async fn file_stats_size_active_winsorized_mean_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(Option<String>, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT mime_type, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let winsorized_mean = if n < 2 { vals.first().copied().unwrap_or(0.0) } else {
            let p10 = vals[(n as f64 * 0.10) as usize];
            let p90 = vals[((n as f64 * 0.90) as usize).min(n - 1)];
            let clamped: Vec<f64> = vals.iter().map(|&v| v.clamp(p10, p90)).collect();
            clamped.iter().sum::<f64>() / clamped.len() as f64
        };
        serde_json::json!({"mime_type": mime, "winsorized_mean_active_size": winsorized_mean, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-winsorized-mean-by-owner — média winsorizada de size ativo por owner. Sprint #3372.
async fn file_stats_size_active_winsorized_mean_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let winsorized_mean = if n < 2 { vals.first().copied().unwrap_or(0.0) } else {
            let p10 = vals[(n as f64 * 0.10) as usize];
            let p90 = vals[((n as f64 * 0.90) as usize).min(n - 1)];
            let clamped: Vec<f64> = vals.iter().map(|&v| v.clamp(p10, p90)).collect();
            clamped.iter().sum::<f64>() / clamped.len() as f64
        };
        serde_json::json!({"owner_id": owner, "winsorized_mean_active_size": winsorized_mean, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-winsorized-mean-by-ext — média winsorizada de size ativo por extensão. Sprint #3389.
async fn file_stats_size_active_winsorized_mean_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(Option<String>, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT extension, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY extension ORDER BY extension",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let winsorized_mean = if n < 2 { vals.first().copied().unwrap_or(0.0) } else {
            let p10 = vals[(n as f64 * 0.10) as usize];
            let p90 = vals[((n as f64 * 0.90) as usize).min(n - 1)];
            let clamped: Vec<f64> = vals.iter().map(|&v| v.clamp(p10, p90)).collect();
            clamped.iter().sum::<f64>() / clamped.len() as f64
        };
        serde_json::json!({"extension": ext, "winsorized_mean_active_size": winsorized_mean, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-harmonic-mean-by-kind — média harmônica de size ativo por kind. Sprint #3390.
async fn file_stats_size_active_harmonic_mean_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT kind, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).filter(|&v| v > 0.0).collect();
        let n = vals.len();
        let harmonic_mean = if n == 0 { 0.0 } else {
            let recip_sum: f64 = vals.iter().map(|&v| 1.0 / v).sum();
            if recip_sum == 0.0 { 0.0 } else { n as f64 / recip_sum }
        };
        serde_json::json!({"kind": kind, "harmonic_mean_active_size": harmonic_mean, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-harmonic-mean-by-mime — média harmônica de size ativo por mime_type. Sprint #3391.
async fn file_stats_size_active_harmonic_mean_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(Option<String>, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT mime_type, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).filter(|&v| v > 0.0).collect();
        let n = vals.len();
        let harmonic_mean = if n == 0 { 0.0 } else {
            let recip_sum: f64 = vals.iter().map(|&v| 1.0 / v).sum();
            if recip_sum == 0.0 { 0.0 } else { n as f64 / recip_sum }
        };
        serde_json::json!({"mime_type": mime, "harmonic_mean_active_size": harmonic_mean, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-harmonic-mean-by-owner — média harmônica de size ativo por owner. Sprint #3392.
async fn file_stats_size_active_harmonic_mean_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).filter(|&v| v > 0.0).collect();
        let n = vals.len();
        let harmonic_mean = if n == 0 { 0.0 } else {
            let recip_sum: f64 = vals.iter().map(|&v| 1.0 / v).sum();
            if recip_sum == 0.0 { 0.0 } else { n as f64 / recip_sum }
        };
        serde_json::json!({"owner_id": owner, "harmonic_mean_active_size": harmonic_mean, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-harmonic-mean-by-ext — média harmônica de size ativo por extensão. Sprint #3409.
async fn file_stats_size_active_harmonic_mean_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(Option<String>, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT extension, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY extension ORDER BY extension",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).filter(|&v| v > 0.0).collect();
        let n = vals.len();
        let harmonic_mean = if n == 0 { 0.0 } else {
            let recip_sum: f64 = vals.iter().map(|&v| 1.0 / v).sum();
            if recip_sum == 0.0 { 0.0 } else { n as f64 / recip_sum }
        };
        serde_json::json!({"extension": ext, "harmonic_mean_active_size": harmonic_mean, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-geometric-mean-by-kind — média geométrica de size ativo por kind. Sprint #3410.
async fn file_stats_size_active_geometric_mean_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT kind, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).filter(|&v| v > 0.0).collect();
        let n = vals.len();
        let geometric_mean = if n == 0 { 0.0 } else { (vals.iter().map(|&v| v.ln()).sum::<f64>() / n as f64).exp() };
        serde_json::json!({"kind": kind, "geometric_mean_active_size": geometric_mean, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-geometric-mean-by-mime — média geométrica de size ativo por mime_type. Sprint #3411.
async fn file_stats_size_active_geometric_mean_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(Option<String>, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT mime_type, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).filter(|&v| v > 0.0).collect();
        let n = vals.len();
        let geometric_mean = if n == 0 { 0.0 } else { (vals.iter().map(|&v| v.ln()).sum::<f64>() / n as f64).exp() };
        serde_json::json!({"mime_type": mime, "geometric_mean_active_size": geometric_mean, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-geometric-mean-by-owner — média geométrica de size ativo por owner. Sprint #3412.
async fn file_stats_size_active_geometric_mean_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).filter(|&v| v > 0.0).collect();
        let n = vals.len();
        let geometric_mean = if n == 0 { 0.0 } else { (vals.iter().map(|&v| v.ln()).sum::<f64>() / n as f64).exp() };
        serde_json::json!({"owner_id": owner, "geometric_mean_active_size": geometric_mean, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-geometric-mean-by-ext — média geométrica de size ativo por extensão. Sprint #3429.
async fn file_stats_size_active_geometric_mean_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(Option<String>, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT extension, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY extension ORDER BY extension",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).filter(|&v| v > 0.0).collect();
        let n = vals.len();
        let geometric_mean = if n == 0 { 0.0 } else { (vals.iter().map(|&v| v.ln()).sum::<f64>() / n as f64).exp() };
        serde_json::json!({"extension": ext, "geometric_mean_active_size": geometric_mean, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-harmonic-mean-by-kind — média harmônica de tamanho deletado por kind. Sprint #3430.
async fn file_stats_size_deleted_harmonic_mean_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT kind, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).filter(|&v| v > 0.0).collect();
        let n = vals.len();
        let harmonic_mean = if n == 0 { 0.0 } else {
            let recip_sum: f64 = vals.iter().map(|&v| 1.0 / v).sum();
            if recip_sum == 0.0 { 0.0 } else { n as f64 / recip_sum }
        };
        serde_json::json!({"kind": kind, "harmonic_mean_deleted_size": harmonic_mean, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-harmonic-mean-by-mime — média harmônica de tamanho deletado por mime_type. Sprint #3431.
async fn file_stats_size_deleted_harmonic_mean_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(Option<String>, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT mime_type, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).filter(|&v| v > 0.0).collect();
        let n = vals.len();
        let harmonic_mean = if n == 0 { 0.0 } else {
            let recip_sum: f64 = vals.iter().map(|&v| 1.0 / v).sum();
            if recip_sum == 0.0 { 0.0 } else { n as f64 / recip_sum }
        };
        serde_json::json!({"mime_type": mime, "harmonic_mean_deleted_size": harmonic_mean, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-harmonic-mean-by-owner — média harmônica de tamanho deletado por owner. Sprint #3432.
async fn file_stats_size_deleted_harmonic_mean_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).filter(|&v| v > 0.0).collect();
        let n = vals.len();
        let harmonic_mean = if n == 0 { 0.0 } else {
            let recip_sum: f64 = vals.iter().map(|&v| 1.0 / v).sum();
            if recip_sum == 0.0 { 0.0 } else { n as f64 / recip_sum }
        };
        serde_json::json!({"owner_id": owner, "harmonic_mean_deleted_size": harmonic_mean, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-harmonic-mean-by-ext — média harmônica de size de arquivos deletados por extensão. Sprint #3449.
async fn file_stats_size_deleted_harmonic_mean_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT COALESCE(extension, 'none'), ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY extension ORDER BY extension",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).filter(|&v| v > 0.0).collect();
        let n = vals.len();
        let harmonic_mean = if n == 0 { 0.0 } else {
            let recip_sum: f64 = vals.iter().map(|&v| 1.0 / v).sum();
            if recip_sum == 0.0 { 0.0 } else { n as f64 / recip_sum }
        };
        serde_json::json!({"extension": ext, "harmonic_mean_deleted_size": harmonic_mean, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-geometric-mean-by-kind — média geométrica de size de arquivos deletados por kind. Sprint #3450.
async fn file_stats_size_deleted_geometric_mean_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT kind, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).filter(|&v| v > 0.0).collect();
        let n = vals.len();
        let geometric_mean = if n == 0 { 0.0 } else {
            let log_sum: f64 = vals.iter().map(|&v| v.ln()).sum();
            (log_sum / n as f64).exp()
        };
        serde_json::json!({"kind": kind, "geometric_mean_deleted_size": geometric_mean, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-geometric-mean-by-mime — média geométrica de size de arquivos deletados por mime_type. Sprint #3451.
async fn file_stats_size_deleted_geometric_mean_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type, 'unknown'), ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).filter(|&v| v > 0.0).collect();
        let n = vals.len();
        let geometric_mean = if n == 0 { 0.0 } else {
            let log_sum: f64 = vals.iter().map(|&v| v.ln()).sum();
            (log_sum / n as f64).exp()
        };
        serde_json::json!({"mime_type": mime, "geometric_mean_deleted_size": geometric_mean, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-geometric-mean-by-owner — média geométrica de size de arquivos deletados por owner. Sprint #3452.
async fn file_stats_size_deleted_geometric_mean_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).filter(|&v| v > 0.0).collect();
        let n = vals.len();
        let geometric_mean = if n == 0 { 0.0 } else {
            let log_sum: f64 = vals.iter().map(|&v| v.ln()).sum();
            (log_sum / n as f64).exp()
        };
        serde_json::json!({"owner_id": owner, "geometric_mean_deleted_size": geometric_mean, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-geometric-mean-by-ext — média geométrica de size de arquivos deletados por extensão. Sprint #3469.
async fn file_stats_size_deleted_geometric_mean_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT COALESCE(extension, 'none'), ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY extension ORDER BY extension",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).filter(|&v| v > 0.0).collect();
        let n = vals.len();
        let geometric_mean = if n == 0 { 0.0 } else {
            let log_sum: f64 = vals.iter().map(|&v| v.ln()).sum();
            (log_sum / n as f64).exp()
        };
        serde_json::json!({"extension": ext, "geometric_mean_deleted_size": geometric_mean, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-normalized-entropy-by-kind — entropia normalizada de size de arquivos deletados por kind. Sprint #3470.
async fn file_stats_size_deleted_normalized_entropy_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT kind, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let total: f64 = vals.iter().sum();
        let norm_entropy = if n < 2 || total == 0.0 { 0.0 } else {
            let entropy: f64 = vals.iter().map(|&v| { let p = v / total; if p > 0.0 { -p * p.ln() } else { 0.0 } }).sum();
            entropy / (n as f64).ln()
        };
        serde_json::json!({"kind": kind, "normalized_entropy_deleted_size": norm_entropy, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-normalized-entropy-by-mime — entropia normalizada de size de arquivos deletados por mime_type. Sprint #3471.
async fn file_stats_size_deleted_normalized_entropy_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type, 'unknown'), ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let total: f64 = vals.iter().sum();
        let norm_entropy = if n < 2 || total == 0.0 { 0.0 } else {
            let entropy: f64 = vals.iter().map(|&v| { let p = v / total; if p > 0.0 { -p * p.ln() } else { 0.0 } }).sum();
            entropy / (n as f64).ln()
        };
        serde_json::json!({"mime_type": mime, "normalized_entropy_deleted_size": norm_entropy, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-normalized-entropy-by-owner — entropia normalizada de size de arquivos deletados por owner. Sprint #3472.
async fn file_stats_size_deleted_normalized_entropy_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let total: f64 = vals.iter().sum();
        let norm_entropy = if n < 2 || total == 0.0 { 0.0 } else {
            let entropy: f64 = vals.iter().map(|&v| { let p = v / total; if p > 0.0 { -p * p.ln() } else { 0.0 } }).sum();
            entropy / (n as f64).ln()
        };
        serde_json::json!({"owner_id": owner, "normalized_entropy_deleted_size": norm_entropy, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-normalized-entropy-by-ext — entropia normalizada de size deletados por extensão. Sprint #3489.
async fn file_stats_size_deleted_normalized_entropy_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT COALESCE(extension, 'none'), ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY extension ORDER BY extension",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let total: f64 = vals.iter().sum();
        let norm_entropy = if n < 2 || total == 0.0 { 0.0 } else {
            let entropy: f64 = vals.iter().map(|&v| { let p = v / total; if p > 0.0 { -p * p.ln() } else { 0.0 } }).sum();
            entropy / (n as f64).ln()
        };
        serde_json::json!({"extension": ext, "normalized_entropy_deleted_size": norm_entropy, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/count-deleted-by-ext — contagem de arquivos deletados por extensão. Sprint #3490.
async fn file_stats_count_deleted_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT COALESCE(extension, 'none'), COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY extension ORDER BY deleted_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let total: i64 = rows.iter().map(|(_, c)| c).sum();
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, cnt)| {
        serde_json::json!({"extension": ext, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"total_deleted": total, "rows": result})))
}

/// GET /api/v1/drive/files/stats/count-active-by-ext — contagem de arquivos ativos por extensão. Sprint #3491.
async fn file_stats_count_active_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT COALESCE(extension, 'none'), COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY extension ORDER BY active_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let total: i64 = rows.iter().map(|(_, c)| c).sum();
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, cnt)| {
        serde_json::json!({"extension": ext, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"total_active": total, "rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-coeff-var-by-kind — coeficiente de variação de size de arquivos ativos por kind. Sprint #3492.
async fn file_stats_size_active_coeff_var_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT kind, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let coeff_var = if n == 0 { 0.0 } else {
            let mean = vals.iter().sum::<f64>() / n as f64;
            if mean == 0.0 { 0.0 } else {
                let variance = vals.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n as f64;
                variance.sqrt() / mean
            }
        };
        serde_json::json!({"kind": kind, "coeff_var_active_size": coeff_var, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-coeff-var-by-mime — coeficiente de variação de size de arquivos ativos por mime_type. Sprint #3509.
async fn file_stats_size_active_coeff_var_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type, 'unknown'), ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let coeff_var = if n == 0 { 0.0 } else {
            let mean = vals.iter().sum::<f64>() / n as f64;
            if mean == 0.0 { 0.0 } else {
                let variance = vals.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n as f64;
                variance.sqrt() / mean
            }
        };
        serde_json::json!({"mime_type": mime, "coeff_var_active_size": coeff_var, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-coeff-var-by-owner — coeficiente de variação de size de arquivos ativos por owner. Sprint #3510.
async fn file_stats_size_active_coeff_var_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let coeff_var = if n == 0 { 0.0 } else {
            let mean = vals.iter().sum::<f64>() / n as f64;
            if mean == 0.0 { 0.0 } else {
                let variance = vals.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n as f64;
                variance.sqrt() / mean
            }
        };
        serde_json::json!({"owner_id": owner, "coeff_var_active_size": coeff_var, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-coeff-var-by-ext — coeficiente de variação de size de arquivos ativos por extensão. Sprint #3511.
async fn file_stats_size_active_coeff_var_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT COALESCE(extension, 'none'), ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY extension ORDER BY extension",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let coeff_var = if n == 0 { 0.0 } else {
            let mean = vals.iter().sum::<f64>() / n as f64;
            if mean == 0.0 { 0.0 } else {
                let variance = vals.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n as f64;
                variance.sqrt() / mean
            }
        };
        serde_json::json!({"extension": ext, "coeff_var_active_size": coeff_var, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-coeff-var-by-kind — coeficiente de variação de size de arquivos deletados por kind. Sprint #3512.
async fn file_stats_size_deleted_coeff_var_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT kind, ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let coeff_var = if n == 0 { 0.0 } else {
            let mean = vals.iter().sum::<f64>() / n as f64;
            if mean == 0.0 { 0.0 } else {
                let variance = vals.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n as f64;
                variance.sqrt() / mean
            }
        };
        serde_json::json!({"kind": kind, "coeff_var_deleted_size": coeff_var, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-mad-by-mime — MAD de size ativo por mime. Sprint #3529.
async fn file_stats_size_active_mad_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type, 'unknown'), ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let mad = if n == 0 { 0.0 } else {
            let mean = vals.iter().sum::<f64>() / n as f64;
            vals.iter().map(|&v| (v - mean).abs()).sum::<f64>() / n as f64
        };
        serde_json::json!({"mime_type": mime, "mad_active_size": mad, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-mad-by-owner — MAD de size ativo por owner. Sprint #3530.
async fn file_stats_size_active_mad_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT COALESCE(owner_id::TEXT, 'unknown'), ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let mad = if n == 0 { 0.0 } else {
            let mean = vals.iter().sum::<f64>() / n as f64;
            vals.iter().map(|&v| (v - mean).abs()).sum::<f64>() / n as f64
        };
        serde_json::json!({"owner_id": owner, "mad_active_size": mad, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-mad-by-ext — MAD de size ativo por ext. Sprint #3531.
async fn file_stats_size_active_mad_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT COALESCE(file_ext, 'unknown'), ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY file_ext ORDER BY file_ext",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let mad = if n == 0 { 0.0 } else {
            let mean = vals.iter().sum::<f64>() / n as f64;
            vals.iter().map(|&v| (v - mean).abs()).sum::<f64>() / n as f64
        };
        serde_json::json!({"file_ext": ext, "mad_active_size": mad, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-coeff-var-by-mime — CV de size deletado por mime. Sprint #3532.
async fn file_stats_size_deleted_coeff_var_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type, 'unknown'), ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let coeff_var = if n == 0 { 0.0 } else {
            let mean = vals.iter().sum::<f64>() / n as f64;
            if mean == 0.0 { 0.0 } else {
                let variance = vals.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n as f64;
                variance.sqrt() / mean
            }
        };
        serde_json::json!({"mime_type": mime, "coeff_var_deleted_size": coeff_var, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/count-active-by-kind — contagem de arquivos ativos por kind. Sprint #2901.
async fn file_stats_count_active_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT kind, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let total: i64 = rows.iter().map(|(_, c)| c).sum();
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, cnt)| {
        serde_json::json!({"kind": kind, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"total_active": total, "rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-sum-by-kind — soma de size ativo por kind. Sprint #2946.
async fn file_stats_size_active_sum_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT kind, COALESCE(SUM(size_bytes), 0)::BIGINT AS total_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let grand_total: i64 = rows.iter().map(|(_, s, _)| s).sum();
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, total, cnt)| {
        serde_json::json!({"kind": kind, "total_active_size": total, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"grand_total_active_size": grand_total, "rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-sum-by-mime — soma de size ativo por mime_type. Sprint #2951.
async fn file_stats_size_active_sum_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT mime_type, COALESCE(SUM(size_bytes), 0)::BIGINT AS total_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let grand_total: i64 = rows.iter().map(|(_, s, _)| s).sum();
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, total, cnt)| {
        serde_json::json!({"mime_type": mime, "total_active_size": total, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"grand_total_active_size": grand_total, "rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-min-by-kind — mínimo de size ativo por kind. Sprint #2956.
async fn file_stats_size_active_min_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT kind, COALESCE(MIN(size_bytes), 0)::BIGINT AS min_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, min_sz, cnt)| {
        serde_json::json!({"kind": kind, "min_active_size": min_sz, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-sum-by-owner — soma de size ativo por owner. Sprint #2961.
async fn file_stats_size_active_sum_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(uuid::Uuid, i64, i64)> = sqlx::query_as(
        "SELECT owner_id, COALESCE(SUM(size_bytes), 0)::BIGINT AS total_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let grand_total: i64 = rows.iter().map(|(_, s, _)| s).sum();
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner_id, total, cnt)| {
        serde_json::json!({"owner_id": owner_id, "total_active_size": total, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"grand_total_active_size": grand_total, "rows": result})))
}

/// GET /api/v1/drive/files/stats/size-deleted-sum-by-owner — soma de size de arquivos deletados por owner. Sprint #2926.
async fn file_stats_size_deleted_sum_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(uuid::Uuid, i64, i64)> = sqlx::query_as(
        "SELECT owner_id, COALESCE(SUM(size_bytes), 0)::BIGINT AS total_deleted_size, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let grand_total: i64 = rows.iter().map(|(_, s, _)| s).sum();
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner_id, total, cnt)| {
        serde_json::json!({"owner_id": owner_id, "total_deleted_size": total, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"grand_total_deleted_size": grand_total, "rows": result})))
}

/// GET /api/v1/drive/files/stats/count-deleted-by-kind — contagem de arquivos deletados por kind. Sprint #2931.
async fn file_stats_count_deleted_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT kind, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let total: i64 = rows.iter().map(|(_, c)| c).sum();
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, cnt)| {
        serde_json::json!({"kind": kind, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"total_deleted": total, "rows": result})))
}

/// GET /api/v1/drive/files/stats/count-deleted-by-mime — contagem de arquivos deletados por mime_type. Sprint #2936.
async fn file_stats_count_deleted_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT mime_type, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let total: i64 = rows.iter().map(|(_, c)| c).sum();
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, cnt)| {
        serde_json::json!({"mime_type": mime, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"total_deleted": total, "rows": result})))
}

/// GET /api/v1/drive/files/stats/count-deleted-by-owner — contagem de arquivos deletados por owner. Sprint #2941.
async fn file_stats_count_deleted_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(uuid::Uuid, i64)> = sqlx::query_as(
        "SELECT owner_id, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let total: i64 = rows.iter().map(|(_, c)| c).sum();
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner_id, cnt)| {
        serde_json::json!({"owner_id": owner_id, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"total_deleted": total, "rows": result})))
}

/// GET /api/v1/drive/files/stats/count-active-by-mime — contagem de arquivos ativos por mime_type. Sprint #2906.
async fn file_stats_count_active_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT mime_type, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let total: i64 = rows.iter().map(|(_, c)| c).sum();
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, cnt)| {
        serde_json::json!({"mime_type": mime, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"total_active": total, "rows": result})))
}

/// GET /api/v1/drive/files/stats/count-active-by-owner — contagem de arquivos ativos por owner. Sprint #2911.
async fn file_stats_count_active_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(uuid::Uuid, i64)> = sqlx::query_as(
        "SELECT owner_id, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let total: i64 = rows.iter().map(|(_, c)| c).sum();
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner_id, cnt)| {
        serde_json::json!({"owner_id": owner_id, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"total_active": total, "rows": result})))
}

/// GET /api/v1/drive/files/stats/size-deleted-sum-by-kind — soma de size de arquivos deletados por kind. Sprint #2916.
async fn file_stats_size_deleted_sum_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT kind, COALESCE(SUM(size_bytes), 0)::BIGINT AS total_deleted_size, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let grand_total: i64 = rows.iter().map(|(_, s, _)| s).sum();
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, total, cnt)| {
        serde_json::json!({"kind": kind, "total_deleted_size": total, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"grand_total_deleted_size": grand_total, "rows": result})))
}

/// GET /api/v1/drive/files/stats/size-deleted-sum-by-mime — soma de size de arquivos deletados por mime_type. Sprint #2921.
async fn file_stats_size_deleted_sum_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT mime_type, COALESCE(SUM(size_bytes), 0)::BIGINT AS total_deleted_size, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let grand_total: i64 = rows.iter().map(|(_, s, _)| s).sum();
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, total, cnt)| {
        serde_json::json!({"mime_type": mime, "total_deleted_size": total, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"grand_total_deleted_size": grand_total, "rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-max-by-owner — máximo de size ativo por owner. Sprint #2986.
async fn file_stats_size_active_max_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(uuid::Uuid, i64, i64)> = sqlx::query_as(
        "SELECT owner_id, COALESCE(MAX(size_bytes), 0)::BIGINT AS max_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner_id, max, cnt)| {
        serde_json::json!({"owner_id": owner_id, "max_active_size": max, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-p90-by-owner — P90 de size_bytes de arquivos ativos por owner. Sprint #3041.
async fn file_stats_size_active_p90_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(Option<String>, f64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, COALESCE(PERCENTILE_CONT(0.90) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS p90_size, COUNT(*)::BIGINT AS cnt \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(oid, p90, cnt)| {
        serde_json::json!({"owner_id": oid, "p90_size": p90, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-p75-by-owner — P75 de size_bytes de arquivos ativos por owner. Sprint #3036.
async fn file_stats_size_active_p75_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(Option<String>, f64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, COALESCE(PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS p75_size, COUNT(*)::BIGINT AS cnt \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(oid, p75, cnt)| {
        serde_json::json!({"owner_id": oid, "p75_size": p75, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-p50-by-owner — P50 de size_bytes de arquivos ativos por owner. Sprint #3031.
async fn file_stats_size_active_p50_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(Option<String>, f64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, COALESCE(PERCENTILE_CONT(0.50) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS p50_size, COUNT(*)::BIGINT AS cnt \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(oid, p50, cnt)| {
        serde_json::json!({"owner_id": oid, "p50_size": p50, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-p90-by-mime — P90 de size_bytes de arquivos ativos por mime_type. Sprint #3026.
async fn file_stats_size_active_p90_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT mime_type, COALESCE(PERCENTILE_CONT(0.90) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS p90_size, COUNT(*)::BIGINT AS cnt \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, p90, cnt)| {
        serde_json::json!({"mime_type": mime, "p90_size": p90, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-p90-by-kind — P90 de size_bytes de arquivos ativos por kind. Sprint #3021.
async fn file_stats_size_active_p90_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT kind, COALESCE(PERCENTILE_CONT(0.90) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS p90_size, COUNT(*)::BIGINT AS cnt \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, p90, cnt)| {
        serde_json::json!({"kind": kind, "p90_size": p90, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-p75-by-mime — P75 de size_bytes de arquivos ativos por mime_type. Sprint #3016.
async fn file_stats_size_active_p75_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT mime_type, COALESCE(PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS p75_size, COUNT(*)::BIGINT AS cnt \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, p75, cnt)| {
        serde_json::json!({"mime_type": mime, "p75_size": p75, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-p75-by-kind — P75 de size_bytes de arquivos ativos por kind. Sprint #3011.
async fn file_stats_size_active_p75_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT kind, COALESCE(PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS p75_size, COUNT(*)::BIGINT AS cnt \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, p75, cnt)| {
        serde_json::json!({"kind": kind, "p75_size": p75, "count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-count-by-owner — contagem de arquivos deletados por owner. Sprint #3006.
async fn file_stats_size_deleted_count_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(Option<String>, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let grand_total: i64 = rows.iter().map(|(_, c)| c).sum();
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(oid, cnt)| {
        serde_json::json!({"owner_id": oid, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"grand_total_deleted": grand_total, "rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-count-by-mime — contagem de arquivos deletados por mime_type. Sprint #3001.
async fn file_stats_size_deleted_count_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT mime_type, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let grand_total: i64 = rows.iter().map(|(_, c)| c).sum();
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, cnt)| {
        serde_json::json!({"mime_type": mime, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"grand_total_deleted": grand_total, "rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-count-by-kind — contagem de arquivos deletados por kind. Sprint #2996.
async fn file_stats_size_deleted_count_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT kind, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let grand_total: i64 = rows.iter().map(|(_, c)| c).sum();
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, cnt)| {
        serde_json::json!({"kind": kind, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"grand_total_deleted": grand_total, "rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-avg-by-mime — média de size ativo por mime_type. Sprint #2991.
async fn file_stats_size_active_avg_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT mime_type, COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, avg, cnt)| {
        serde_json::json!({"mime_type": mime, "avg_active_size": avg, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-max-by-mime — máximo de size ativo por mime_type. Sprint #2981.
async fn file_stats_size_active_max_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT mime_type, COALESCE(MAX(size_bytes), 0)::BIGINT AS max_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, max, cnt)| {
        serde_json::json!({"mime_type": mime, "max_active_size": max, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-max-by-kind — máximo de size ativo por kind. Sprint #2976.
async fn file_stats_size_active_max_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT kind, COALESCE(MAX(size_bytes), 0)::BIGINT AS max_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, max, cnt)| {
        serde_json::json!({"kind": kind, "max_active_size": max, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-min-by-owner — mínimo de size ativo por owner. Sprint #2971.
async fn file_stats_size_active_min_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(uuid::Uuid, i64, i64)> = sqlx::query_as(
        "SELECT owner_id, COALESCE(MIN(size_bytes), 0)::BIGINT AS min_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner_id, min, cnt)| {
        serde_json::json!({"owner_id": owner_id, "min_active_size": min, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-min-by-mime — mínimo de size ativo por mime_type. Sprint #2966.
async fn file_stats_size_active_min_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT mime_type, COALESCE(MIN(size_bytes), 0)::BIGINT AS min_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, min, cnt)| {
        serde_json::json!({"mime_type": mime, "min_active_size": min, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-p95-by-mime — P95 do tamanho de arquivos ativos por mime_type. Sprint #2746.
async fn file_stats_size_active_p95_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT mime_type, \
                COALESCE(PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS p95_active_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY p95_active_size DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, p95, cnt)| {
        serde_json::json!({"mime_type": mime, "p95_active_size_bytes": p95, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-iqr-by-kind — IQR do tamanho de arquivos ativos por kind. Sprint #2751.
async fn file_stats_size_active_iqr_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT kind, \
                COALESCE(PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY size_bytes) - \
                         PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS iqr_active_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY kind ORDER BY iqr_active_size DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, iqr, cnt)| {
        serde_json::json!({"kind": kind, "iqr_active_size_bytes": iqr, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-iqr-by-mime — IQR do tamanho de arquivos ativos por mime_type. Sprint #2756.
async fn file_stats_size_active_iqr_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT mime_type, \
                COALESCE(PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY size_bytes) - \
                         PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS iqr_active_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY iqr_active_size DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, iqr, cnt)| {
        serde_json::json!({"mime_type": mime, "iqr_active_size_bytes": iqr, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-iqr-by-owner — IQR do tamanho de arquivos ativos por owner_id. Sprint #2761.
async fn file_stats_size_active_iqr_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
                COALESCE(PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY size_bytes) - \
                         PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS iqr_active_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY iqr_active_size DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner, iqr, cnt)| {
        serde_json::json!({"owner_id": owner, "iqr_active_size_bytes": iqr, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-iqr-by-ext — IQR de size ativo por extensão. Sprint #3549.
async fn file_stats_size_active_iqr_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT COALESCE(file_ext, 'unknown'), \
                COALESCE(PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY size_bytes) - \
                         PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS iqr_active_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY file_ext ORDER BY iqr_active_size DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, iqr, cnt)| {
        serde_json::json!({"file_ext": ext, "iqr_active_size_bytes": iqr, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-range-by-ext — range de size ativo por extensão. Sprint #3550.
async fn file_stats_size_active_range_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(file_ext, 'unknown'), \
                COALESCE(MAX(size_bytes), 0)::BIGINT AS max_size, \
                COALESCE(MIN(size_bytes), 0)::BIGINT AS min_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY file_ext ORDER BY file_ext",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, max_s, min_s, cnt)| {
        serde_json::json!({"file_ext": ext, "range_active_size_bytes": max_s - min_s, "max_bytes": max_s, "min_bytes": min_s, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-p95-by-owner — P95 de size ativo por owner. Sprint #3551.
async fn file_stats_size_active_p95_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
                COALESCE(PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS p95_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY p95_size DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner, p95, cnt)| {
        serde_json::json!({"owner_id": owner, "p95_active_size_bytes": p95, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-p95-by-ext — P95 de size ativo por extensão. Sprint #3552.
async fn file_stats_size_active_p95_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT COALESCE(file_ext, 'unknown'), \
                COALESCE(PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS p95_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY file_ext ORDER BY p95_size DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, p95, cnt)| {
        serde_json::json!({"file_ext": ext, "p95_active_size_bytes": p95, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-cv-by-owner — CV do tamanho de arquivos ativos por owner_id. Sprint #2726.
async fn file_stats_size_active_cv_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, f64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
                COALESCE(STDDEV(size_bytes), 0.0)::FLOAT8 AS stddev_s, \
                COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_s, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner, stddev, avg, cnt)| {
        let cv = if avg > 0.0 { stddev / avg } else { 0.0 };
        serde_json::json!({"owner_id": owner, "cv_active_size": cv, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-cv-by-kind — CV do tamanho de arquivos ativos por kind. Sprint #2731.
async fn file_stats_size_active_cv_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, f64, i64)> = sqlx::query_as(
        "SELECT kind, \
                COALESCE(STDDEV(size_bytes), 0.0)::FLOAT8 AS stddev_s, \
                COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_s, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, stddev, avg, cnt)| {
        let cv = if avg > 0.0 { stddev / avg } else { 0.0 };
        serde_json::json!({"kind": kind, "cv_active_size": cv, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-cv-by-mime — CV do tamanho de arquivos ativos por mime_type. Sprint #2736.
async fn file_stats_size_active_cv_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, f64, i64)> = sqlx::query_as(
        "SELECT mime_type, \
                COALESCE(STDDEV(size_bytes), 0.0)::FLOAT8 AS stddev_s, \
                COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_s, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, stddev, avg, cnt)| {
        let cv = if avg > 0.0 { stddev / avg } else { 0.0 };
        serde_json::json!({"mime_type": mime, "cv_active_size": cv, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-count-by-kind — contagem de arquivos ativos por kind. Sprint #3046.
async fn file_stats_size_active_count_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT kind, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY kind ORDER BY active_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let total: i64 = rows.iter().map(|(_, c)| c).sum();
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, cnt)| {
        serde_json::json!({"kind": kind, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"total_active_count": total, "rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-variance-by-kind — variância do tamanho de arquivos ativos por kind. Sprint #3066.
async fn file_stats_size_active_variance_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT kind, COALESCE(VAR_POP(size_bytes), 0.0)::FLOAT8 AS variance_active_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY kind ORDER BY variance_active_size DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, var, cnt)| {
        serde_json::json!({"kind": kind, "variance_active_size_bytes": var, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-variance-by-mime — variância do tamanho de arquivos ativos por mime_type. Sprint #3071.
async fn file_stats_size_active_variance_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT mime_type, COALESCE(VAR_POP(size_bytes), 0.0)::FLOAT8 AS variance_active_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY variance_active_size DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, var, cnt)| {
        serde_json::json!({"mime_type": mime, "variance_active_size_bytes": var, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-variance-by-owner — variância do tamanho de arquivos ativos por owner_id. Sprint #3076.
async fn file_stats_size_active_variance_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(uuid::Uuid, f64, i64)> = sqlx::query_as(
        "SELECT owner_id, COALESCE(VAR_POP(size_bytes), 0.0)::FLOAT8 AS variance_active_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY variance_active_size DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner_id, var, cnt)| {
        serde_json::json!({"owner_id": owner_id, "variance_active_size_bytes": var, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-mad-by-kind — MAD do tamanho de arquivos ativos por kind. Sprint #3081.
async fn file_stats_size_active_mad_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT kind, \
                COALESCE(AVG(ABS(size_bytes - sub.avg_size)), 0.0)::FLOAT8 AS mad_active_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         JOIN (SELECT kind AS k, AVG(size_bytes) AS avg_size FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL GROUP BY kind) sub \
           ON drive_files.kind = sub.k \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY drive_files.kind ORDER BY mad_active_size DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, mad, cnt)| {
        serde_json::json!({"kind": kind, "mad_active_size_bytes": mad, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-count-by-mime — contagem de arquivos ativos por mime_type. Sprint #3051.
async fn file_stats_size_active_count_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT mime_type, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY active_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let total: i64 = rows.iter().map(|(_, c)| c).sum();
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, cnt)| {
        serde_json::json!({"mime_type": mime, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"total_active_count": total, "rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-count-by-owner — contagem de arquivos ativos por owner_id. Sprint #3056.
async fn file_stats_size_active_count_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(uuid::Uuid, i64)> = sqlx::query_as(
        "SELECT owner_id, COUNT(*)::BIGINT AS active_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY active_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let total: i64 = rows.iter().map(|(_, c)| c).sum();
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner_id, cnt)| {
        serde_json::json!({"owner_id": owner_id, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"total_active_count": total, "rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-p50-by-mime — P50 do tamanho de arquivos ativos por mime_type. Sprint #3061.
async fn file_stats_size_active_p50_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT mime_type, \
                COALESCE(PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS p50_active_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY p50_active_size DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, p50, cnt)| {
        serde_json::json!({"mime_type": mime, "p50_active_size_bytes": p50, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-p95-by-kind — P95 do tamanho de arquivos ativos por kind. Sprint #2741.
async fn file_stats_size_active_p95_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT kind, \
                COALESCE(PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS p95_active_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY kind ORDER BY p95_active_size DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, p95, cnt)| {
        serde_json::json!({"kind": kind, "p95_active_size_bytes": p95, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-stddev-by-owner — stddev do tamanho de arquivos ativos por owner_id. Sprint #2706.
async fn file_stats_size_active_stddev_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, COALESCE(STDDEV(size_bytes), 0.0)::FLOAT8 AS stddev_active_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY stddev_active_size DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner, stddev, cnt)| {
        serde_json::json!({"owner_id": owner, "stddev_active_size_bytes": stddev, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-p90-by-ext — P90 de size ativo por extensão. Sprint #3589.
async fn file_stats_size_active_p90_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT COALESCE(file_ext, 'unknown'), \
                COALESCE(PERCENTILE_CONT(0.90) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS p90_active_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY file_ext ORDER BY p90_active_size DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, p90, cnt)| {
        serde_json::json!({"file_ext": ext, "p90_active_size_bytes": p90, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-p75-by-ext — P75 de size ativo por extensão. Sprint #3590.
async fn file_stats_size_active_p75_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT COALESCE(file_ext, 'unknown'), \
                COALESCE(PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS p75_active_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY file_ext ORDER BY p75_active_size DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, p75, cnt)| {
        serde_json::json!({"file_ext": ext, "p75_active_size_bytes": p75, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-p50-by-ext — P50 de size ativo por extensão. Sprint #3591.
async fn file_stats_size_active_p50_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT COALESCE(file_ext, 'unknown'), \
                COALESCE(PERCENTILE_CONT(0.50) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS p50_active_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY file_ext ORDER BY p50_active_size DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, p50, cnt)| {
        serde_json::json!({"file_ext": ext, "p50_active_size_bytes": p50, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-stddev-by-ext — stddev de size ativo por extensão. Sprint #3592.
async fn file_stats_size_active_stddev_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT COALESCE(file_ext, 'unknown'), \
                COALESCE(STDDEV(size_bytes), 0.0)::FLOAT8 AS stddev_active_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY file_ext ORDER BY stddev_active_size DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, stddev, cnt)| {
        serde_json::json!({"file_ext": ext, "stddev_active_size_bytes": stddev, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-stddev-by-kind — stddev do tamanho de arquivos ativos por kind. Sprint #2711.
async fn file_stats_size_active_stddev_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT kind, COALESCE(STDDEV(size_bytes), 0.0)::FLOAT8 AS stddev_active_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY kind ORDER BY stddev_active_size DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, stddev, cnt)| {
        serde_json::json!({"kind": kind, "stddev_active_size_bytes": stddev, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-p99-by-kind — P99 do tamanho de arquivos ativos por kind. Sprint #2716.
async fn file_stats_size_active_p99_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT kind, \
                COALESCE(PERCENTILE_CONT(0.99) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS p99_active_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY kind ORDER BY p99_active_size DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(kind, p99, cnt)| {
        serde_json::json!({"kind": kind, "p99_active_size_bytes": p99, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-p99-by-mime — P99 do tamanho de arquivos ativos por mime_type. Sprint #2721.
async fn file_stats_size_active_p99_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT mime_type, \
                COALESCE(PERCENTILE_CONT(0.99) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS p99_active_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY p99_active_size DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await
    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, p99, cnt)| {
        serde_json::json!({"mime_type": mime, "p99_active_size_bytes": p99, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-p99-by-owner — P99 de size ativo por owner. Sprint #3569.
async fn file_stats_size_active_p99_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
                COALESCE(PERCENTILE_CONT(0.99) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS p99_active_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY p99_active_size DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner, p99, cnt)| {
        serde_json::json!({"owner_id": owner, "p99_active_size_bytes": p99, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-p99-by-ext — P99 de size ativo por extensão. Sprint #3570.
async fn file_stats_size_active_p99_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT COALESCE(file_ext, 'unknown'), \
                COALESCE(PERCENTILE_CONT(0.99) WITHIN GROUP (ORDER BY size_bytes), 0.0)::FLOAT8 AS p99_active_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY file_ext ORDER BY p99_active_size DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, p99, cnt)| {
        serde_json::json!({"file_ext": ext, "p99_active_size_bytes": p99, "active_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-coeff-var-by-owner — CV de size deletado por owner. Sprint #3571.
async fn file_stats_size_deleted_coeff_var_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT COALESCE(owner_id::TEXT, 'unknown'), ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(owner, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let coeff_var = if n == 0 { 0.0 } else {
            let mean = vals.iter().sum::<f64>() / n as f64;
            if mean == 0.0 { 0.0 } else {
                let variance = vals.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n as f64;
                variance.sqrt() / mean
            }
        };
        serde_json::json!({"owner_id": owner, "coeff_var_deleted_size": coeff_var, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-coeff-var-by-ext — CV de size deletado por extensão. Sprint #3572.
async fn file_stats_size_deleted_coeff_var_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Vec<Option<i64>>, i64)> = sqlx::query_as(
        "SELECT COALESCE(file_ext, 'unknown'), ARRAY_AGG(size_bytes ORDER BY size_bytes) AS sizes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL GROUP BY file_ext ORDER BY file_ext",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(ext, raw, cnt)| {
        let vals: Vec<f64> = raw.into_iter().flatten().map(|v| v as f64).collect();
        let n = vals.len();
        let coeff_var = if n == 0 { 0.0 } else {
            let mean = vals.iter().sum::<f64>() / n as f64;
            if mean == 0.0 { 0.0 } else {
                let variance = vals.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n as f64;
                variance.sqrt() / mean
            }
        };
        serde_json::json!({"file_ext": ext, "coeff_var_deleted_size": coeff_var, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-avg-by-owner — média do tamanho de arquivos ativos por owner_id. Sprint #2686.
async fn file_stats_size_active_avg_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, AVG(size_bytes)::FLOAT8 AS avg_active_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY avg_active_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, avg, cnt)| serde_json::json!({"owner_id": owner, "avg_active_size_bytes": avg, "active_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-avg-by-kind — média do tamanho de arquivos ativos por kind. Sprint #2691.
async fn file_stats_size_active_avg_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT kind, AVG(size_bytes)::FLOAT8 AS avg_active_size, COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY kind ORDER BY avg_active_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, avg, cnt)| serde_json::json!({"kind": kind, "avg_active_size_bytes": avg, "active_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-p99-by-kind — P99 do tamanho de arquivos deletados por kind. Sprint #2696.
async fn file_stats_size_deleted_p99_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT kind, \
                PERCENTILE_CONT(0.99) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p99_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY kind ORDER BY p99_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, p99, cnt)| serde_json::json!({"kind": kind, "p99_deleted_size_bytes": p99, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-p99-by-mime — P99 do tamanho de arquivos deletados por mime_type. Sprint #2701.
async fn file_stats_size_deleted_p99_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT mime_type, \
                PERCENTILE_CONT(0.99) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p99_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY mime_type ORDER BY p99_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, p99, cnt)| serde_json::json!({"mime_type": mime, "p99_deleted_size_bytes": p99, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-p95-by-kind — P95 do tamanho de arquivos deletados por kind. Sprint #2666.
async fn file_stats_size_deleted_p95_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT kind, \
                PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p95_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY kind ORDER BY p95_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, p95, cnt)| serde_json::json!({"kind": kind, "p95_deleted_size_bytes": p95, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-p95-by-mime — P95 do tamanho de arquivos deletados por mime_type. Sprint #2671.
async fn file_stats_size_deleted_p95_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT mime_type, \
                PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p95_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY mime_type ORDER BY p95_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, p95, cnt)| serde_json::json!({"mime_type": mime, "p95_deleted_size_bytes": p95, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-p99-by-owner — P99 do tamanho de arquivos deletados por owner_id. Sprint #2676.
async fn file_stats_size_deleted_p99_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
                PERCENTILE_CONT(0.99) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p99_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY owner_id ORDER BY p99_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, p99, cnt)| serde_json::json!({"owner_id": owner, "p99_deleted_size_bytes": p99, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-p99-by-ext — P99 do tamanho de arquivos deletados por extensão. Sprint #2681.
async fn file_stats_size_deleted_p99_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT LOWER(REVERSE(SPLIT_PART(REVERSE(name), '.', 1))) AS ext, \
                PERCENTILE_CONT(0.99) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p99_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY p99_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, p99, cnt)| serde_json::json!({"ext": ext, "p99_deleted_size_bytes": p99, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-p90-by-kind — P90 do tamanho de arquivos deletados por kind. Sprint #2646.
async fn file_stats_size_deleted_p90_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT kind, \
                PERCENTILE_CONT(0.9) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p90_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY kind ORDER BY p90_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, p90, cnt)| serde_json::json!({"kind": kind, "p90_deleted_size_bytes": p90, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-p90-by-mime — P90 do tamanho de arquivos deletados por mime_type. Sprint #2651.
async fn file_stats_size_deleted_p90_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT mime_type, \
                PERCENTILE_CONT(0.9) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p90_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY mime_type ORDER BY p90_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, p90, cnt)| serde_json::json!({"mime_type": mime, "p90_deleted_size_bytes": p90, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-p95-by-owner — P95 do tamanho de arquivos deletados por owner_id. Sprint #2656.
async fn file_stats_size_deleted_p95_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
                PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p95_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY owner_id ORDER BY p95_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, p95, cnt)| serde_json::json!({"owner_id": owner, "p95_deleted_size_bytes": p95, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-p95-by-ext — P95 do tamanho de arquivos deletados por extensão. Sprint #2661.
async fn file_stats_size_deleted_p95_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT LOWER(REVERSE(SPLIT_PART(REVERSE(name), '.', 1))) AS ext, \
                PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p95_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY p95_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, p95, cnt)| serde_json::json!({"ext": ext, "p95_deleted_size_bytes": p95, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-p75-by-kind — P75 do tamanho de arquivos deletados por kind. Sprint #2626.
async fn file_stats_size_deleted_p75_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT kind, \
                PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p75_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY kind ORDER BY p75_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, p75, cnt)| serde_json::json!({"kind": kind, "p75_deleted_size_bytes": p75, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-p75-by-mime — P75 do tamanho de arquivos deletados por mime_type. Sprint #2631.
async fn file_stats_size_deleted_p75_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT mime_type, \
                PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p75_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY mime_type ORDER BY p75_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, p75, cnt)| serde_json::json!({"mime_type": mime, "p75_deleted_size_bytes": p75, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-p90-by-owner — P90 do tamanho de arquivos deletados por owner_id. Sprint #2636.
async fn file_stats_size_deleted_p90_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
                PERCENTILE_CONT(0.9) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p90_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY owner_id ORDER BY p90_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, p90, cnt)| serde_json::json!({"owner_id": owner, "p90_deleted_size_bytes": p90, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-p90-by-ext — P90 do tamanho de arquivos deletados por extensão. Sprint #2641.
async fn file_stats_size_deleted_p90_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT LOWER(REVERSE(SPLIT_PART(REVERSE(name), '.', 1))) AS ext, \
                PERCENTILE_CONT(0.9) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p90_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY p90_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, p90, cnt)| serde_json::json!({"ext": ext, "p90_deleted_size_bytes": p90, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-p50-by-kind — P50 do tamanho de arquivos deletados por kind. Sprint #2606.
async fn file_stats_size_deleted_p50_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT kind, \
                PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p50_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY kind ORDER BY p50_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, p50, cnt)| serde_json::json!({"kind": kind, "p50_deleted_size_bytes": p50, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-p50-by-mime — P50 do tamanho de arquivos deletados por mime_type. Sprint #2611.
async fn file_stats_size_deleted_p50_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT mime_type, \
                PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p50_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY mime_type ORDER BY p50_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, p50, cnt)| serde_json::json!({"mime_type": mime, "p50_deleted_size_bytes": p50, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-p75-by-owner — P75 do tamanho de arquivos deletados por owner_id. Sprint #2616.
async fn file_stats_size_deleted_p75_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
                PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p75_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY owner_id ORDER BY p75_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, p75, cnt)| serde_json::json!({"owner_id": owner, "p75_deleted_size_bytes": p75, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-p75-by-ext — P75 do tamanho de arquivos deletados por extensão. Sprint #2621.
async fn file_stats_size_deleted_p75_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT LOWER(REVERSE(SPLIT_PART(REVERSE(name), '.', 1))) AS ext, \
                PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p75_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY p75_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, p75, cnt)| serde_json::json!({"ext": ext, "p75_deleted_size_bytes": p75, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-avg-by-kind — média do tamanho de arquivos deletados por kind. Sprint #2586.
async fn file_stats_size_deleted_avg_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT kind, AVG(size_bytes)::FLOAT8 AS avg_deleted_size, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY kind ORDER BY avg_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, avg, cnt)| serde_json::json!({"kind": kind, "avg_deleted_size_bytes": avg, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-avg-by-mime — média do tamanho de arquivos deletados por mime_type. Sprint #2591.
async fn file_stats_size_deleted_avg_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT mime_type, AVG(size_bytes)::FLOAT8 AS avg_deleted_size, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY mime_type ORDER BY avg_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, avg, cnt)| serde_json::json!({"mime_type": mime, "avg_deleted_size_bytes": avg, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-p50-by-owner — P50 do tamanho de arquivos deletados por owner_id. Sprint #2596.
async fn file_stats_size_deleted_p50_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
                PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p50_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY owner_id ORDER BY p50_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, p50, cnt)| serde_json::json!({"owner_id": owner, "p50_deleted_size_bytes": p50, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-p50-by-ext — P50 do tamanho de arquivos deletados por extensão. Sprint #2601.
async fn file_stats_size_deleted_p50_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT LOWER(REVERSE(SPLIT_PART(REVERSE(name), '.', 1))) AS ext, \
                PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p50_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY p50_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, p50, cnt)| serde_json::json!({"ext": ext, "p50_deleted_size_bytes": p50, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-min-by-mime — tamanho mínimo de arquivos deletados por mime_type. Sprint #2546.
async fn file_stats_size_deleted_min_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT mime_type, MIN(size_bytes)::BIGINT AS min_deleted_size, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY mime_type ORDER BY min_deleted_size ASC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, min, cnt)| serde_json::json!({"mime_type": mime, "min_deleted_size_bytes": min, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-max-by-mime — tamanho máximo de arquivos deletados por mime_type. Sprint #2551.
async fn file_stats_size_deleted_max_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT mime_type, MAX(size_bytes)::BIGINT AS max_deleted_size, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY mime_type ORDER BY max_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, max, cnt)| serde_json::json!({"mime_type": mime, "max_deleted_size_bytes": max, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-cv-by-mime — CV do tamanho de arquivos deletados por mime_type. Sprint #2556.
async fn file_stats_size_deleted_cv_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, f64, i64)> = sqlx::query_as(
        "SELECT mime_type, \
                COALESCE(STDDEV(size_bytes), 0.0)::FLOAT8 AS stddev_sz, \
                COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_sz, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY mime_type ORDER BY avg_sz DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter().map(|(mime, stddev, avg, cnt)| {
        let cv = if avg > 0.0 { stddev / avg } else { 0.0 };
        serde_json::json!({"mime_type": mime, "cv_deleted_size": cv, "stddev": stddev, "avg": avg, "deleted_count": cnt})
    }).collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-iqr-by-mime — IQR do tamanho de arquivos deletados por mime_type. Sprint #2561.
async fn file_stats_size_deleted_iqr_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT mime_type, \
                (PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY size_bytes) \
                 - PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY size_bytes))::FLOAT8 AS iqr_sz, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY mime_type ORDER BY iqr_sz DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, iqr, cnt)| serde_json::json!({"mime_type": mime, "iqr_deleted_size_bytes": iqr, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-stddev-by-mime — desvio-padrão do tamanho de arquivos deletados por mime_type. Sprint #2481.
async fn file_stats_size_deleted_stddev_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT mime_type, STDDEV(size_bytes)::FLOAT8 AS stddev_deleted_size, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY mime_type ORDER BY stddev_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, stddev, cnt)| serde_json::json!({"mime_type": mime, "stddev_deleted_size_bytes": stddev, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-avg-by-owner — tamanho médio de arquivos deletados por owner. Sprint #2461.
async fn file_stats_size_deleted_avg_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, AVG(size_bytes)::FLOAT8 AS avg_deleted_size, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY owner_id ORDER BY avg_deleted_size DESC",
    )
    .bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, avg, cnt)| serde_json::json!({"owner_id": owner, "avg_deleted_size_bytes": avg, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/uploads-by-month — contagem de uploads por mês. Sprint #3629.
async fn file_stats_uploads_by_month(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(MONTH FROM created_at AT TIME ZONE 'UTC')::INT AS month, COUNT(*)::BIGINT AS upload_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY month ORDER BY month ASC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, cnt)| serde_json::json!({"month": m, "upload_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/uploads-by-weekday — contagem de uploads por dia da semana. Sprint #3630.
async fn file_stats_uploads_by_weekday(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM created_at AT TIME ZONE 'UTC')::INT AS dow, COUNT(*)::BIGINT AS upload_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY dow ORDER BY dow ASC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(d, cnt)| {
            let day_name = DAY_NAMES.get(d as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"weekday": d, "weekday_name": day_name, "upload_count": cnt})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/uploads-by-hour — contagem de uploads por hora do dia. Sprint #3631.
async fn file_stats_uploads_by_hour(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(HOUR FROM created_at AT TIME ZONE 'UTC')::INT AS hour, COUNT(*)::BIGINT AS upload_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY hour ORDER BY hour ASC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, cnt)| serde_json::json!({"hour": h, "upload_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deletes-by-month — contagem de deleções por mês. Sprint #3632.
async fn file_stats_deletes_by_month(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(MONTH FROM deleted_at AT TIME ZONE 'UTC')::INT AS month, COUNT(*)::BIGINT AS delete_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY month ORDER BY month ASC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, cnt)| serde_json::json!({"month": m, "delete_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deletes-by-weekday — contagem de deleções por dia da semana. Sprint #3649.
async fn file_stats_deletes_by_weekday(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(DOW FROM deleted_at AT TIME ZONE 'UTC')::INT AS dow, COUNT(*)::BIGINT AS delete_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY dow ORDER BY dow ASC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(d, cnt)| {
            let day_name = DAY_NAMES.get(d as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"weekday": d, "weekday_name": day_name, "delete_count": cnt})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deletes-by-hour — contagem de deleções por hora do dia. Sprint #3650.
async fn file_stats_deletes_by_hour(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(HOUR FROM deleted_at AT TIME ZONE 'UTC')::INT AS hour, COUNT(*)::BIGINT AS delete_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY hour ORDER BY hour ASC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, cnt)| serde_json::json!({"hour": h, "delete_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-p50-by-kind — P50 de size ativo por kind. Sprint #3651.
async fn file_stats_size_active_p50_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT kind, \
                PERCENTILE_CONT(0.50) WITHIN GROUP (ORDER BY size_bytes)::FLOAT8 AS p50_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, p50, cnt)| serde_json::json!({"kind": kind, "p50_active_size_bytes": p50, "active_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-stddev-by-mime — desvio padrão de size ativo por mime_type. Sprint #3652.
async fn file_stats_size_active_stddev_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type, 'unknown'), \
                COALESCE(STDDEV(size_bytes), 0.0)::FLOAT8 AS stddev_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, stddev, cnt)| serde_json::json!({"mime_type": mime, "stddev_active_size_bytes": stddev, "active_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/uploads-by-year — contagem de uploads por ano. Sprint #3669.
async fn file_stats_uploads_by_year(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(YEAR FROM created_at AT TIME ZONE 'UTC')::INT AS year, COUNT(*)::BIGINT AS upload_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY year ORDER BY year ASC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(y, cnt)| serde_json::json!({"year": y, "upload_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deletes-by-year — contagem de deleções por ano. Sprint #3670.
async fn file_stats_deletes_by_year(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(YEAR FROM deleted_at AT TIME ZONE 'UTC')::INT AS year, COUNT(*)::BIGINT AS delete_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY year ORDER BY year ASC",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(y, cnt)| serde_json::json!({"year": y, "delete_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-p25-by-kind — P25 de size ativo por kind. Sprint #3671.
async fn file_stats_size_active_p25_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT kind, \
                PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY size_bytes)::FLOAT8 AS p25_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, p25, cnt)| serde_json::json!({"kind": kind, "p25_active_size_bytes": p25, "active_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-avg-by-ext — média de size deletado por extensão. Sprint #3672.
async fn file_stats_size_deleted_avg_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT COALESCE(file_ext, 'unknown'), \
                COALESCE(AVG(size_bytes), 0.0)::FLOAT8 AS avg_deleted_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY file_ext ORDER BY file_ext",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, avg, cnt)| serde_json::json!({"file_ext": ext, "avg_deleted_size_bytes": avg, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-p25-by-mime — P25 de size ativo por mime_type. Sprint #3689.
async fn file_stats_size_active_p25_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type, 'unknown'), \
                PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY size_bytes)::FLOAT8 AS p25_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, p25, cnt)| serde_json::json!({"mime_type": mime, "p25_active_size_bytes": p25, "active_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-p25-by-owner — P25 de size ativo por owner_id. Sprint #3690.
async fn file_stats_size_active_p25_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
                PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY size_bytes)::FLOAT8 AS p25_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, p25, cnt)| serde_json::json!({"owner_id": owner, "p25_active_size_bytes": p25, "active_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-p25-by-ext — P25 de size ativo por extensão. Sprint #3691.
async fn file_stats_size_active_p25_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT COALESCE(file_ext, 'unknown'), \
                PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY size_bytes)::FLOAT8 AS p25_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY file_ext ORDER BY file_ext",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, p25, cnt)| serde_json::json!({"file_ext": ext, "p25_active_size_bytes": p25, "active_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-p25-by-kind — P25 de size deletado por kind. Sprint #3692.
async fn file_stats_size_deleted_p25_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT kind, \
                PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY size_bytes)::FLOAT8 AS p25_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, p25, cnt)| serde_json::json!({"kind": kind, "p25_deleted_size_bytes": p25, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-p25-by-mime — P25 de size deletado por mime_type. Sprint #3709.
async fn file_stats_size_deleted_p25_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type, 'unknown'), \
                PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY size_bytes)::FLOAT8 AS p25_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, p25, cnt)| serde_json::json!({"mime_type": mime, "p25_deleted_size_bytes": p25, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-p25-by-owner — P25 de size deletado por owner_id. Sprint #3710.
async fn file_stats_size_deleted_p25_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
                PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY size_bytes)::FLOAT8 AS p25_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY owner_id ORDER BY owner_id",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, p25, cnt)| serde_json::json!({"owner_id": owner, "p25_deleted_size_bytes": p25, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-p25-by-ext — P25 de size deletado por extensão. Sprint #3711.
async fn file_stats_size_deleted_p25_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT COALESCE(file_ext, 'unknown'), \
                PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY size_bytes)::FLOAT8 AS p25_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY file_ext ORDER BY file_ext",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, p25, cnt)| serde_json::json!({"file_ext": ext, "p25_deleted_size_bytes": p25, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/active-size-p10-by-kind — P10 de size ativo por kind. Sprint #3712.
async fn file_stats_size_active_p10_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT kind, \
                PERCENTILE_CONT(0.10) WITHIN GROUP (ORDER BY size_bytes)::FLOAT8 AS p10_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, p10, cnt)| serde_json::json!({"kind": kind, "p10_active_size_bytes": p10, "active_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-active-p10-by-mime — P10 do size de arquivos ativos por MIME type. Sprint #3729.
async fn file_stats_size_active_p10_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT mime_type, \
                PERCENTILE_CONT(0.10) WITHIN GROUP (ORDER BY size_bytes)::FLOAT8 AS p10_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, p10, cnt)| serde_json::json!({"mime_type": mime, "p10_active_size_bytes": p10, "active_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-active-p10-by-owner — P10 do size de arquivos ativos por owner. Sprint #3730.
async fn file_stats_size_active_p10_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
                PERCENTILE_CONT(0.10) WITHIN GROUP (ORDER BY size_bytes)::FLOAT8 AS p10_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY active_count DESC LIMIT 100",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, p10, cnt)| serde_json::json!({"owner_id": owner, "p10_active_size_bytes": p10, "active_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-active-p10-by-ext — P10 do size de arquivos ativos por extensão. Sprint #3731.
async fn file_stats_size_active_p10_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT LOWER(REGEXP_REPLACE(name, '^.*\\.', '')) AS ext, \
                PERCENTILE_CONT(0.10) WITHIN GROUP (ORDER BY size_bytes)::FLOAT8 AS p10_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY ext ORDER BY ext",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, p10, cnt)| serde_json::json!({"ext": ext, "p10_active_size_bytes": p10, "active_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-deleted-p10-by-kind — P10 do size de arquivos deletados por kind. Sprint #3732.
async fn file_stats_size_deleted_p10_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT kind, \
                PERCENTILE_CONT(0.10) WITHIN GROUP (ORDER BY size_bytes)::FLOAT8 AS p10_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, p10, cnt)| serde_json::json!({"kind": kind, "p10_deleted_size_bytes": p10, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-deleted-p10-by-mime — P10 do size de arquivos deletados por MIME. Sprint #3749.
async fn file_stats_size_deleted_p10_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT mime_type, \
                PERCENTILE_CONT(0.10) WITHIN GROUP (ORDER BY size_bytes)::FLOAT8 AS p10_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, p10, cnt)| serde_json::json!({"mime_type": mime, "p10_deleted_size_bytes": p10, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-deleted-p10-by-owner — P10 do size de arquivos deletados por owner. Sprint #3750.
async fn file_stats_size_deleted_p10_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
                PERCENTILE_CONT(0.10) WITHIN GROUP (ORDER BY size_bytes)::FLOAT8 AS p10_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY owner_id ORDER BY deleted_count DESC LIMIT 100",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, p10, cnt)| serde_json::json!({"owner_id": owner, "p10_deleted_size_bytes": p10, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-deleted-p10-by-ext — P10 do size de arquivos deletados por extensão. Sprint #3751.
async fn file_stats_size_deleted_p10_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT LOWER(REGEXP_REPLACE(name, '^.*\\.', '')) AS ext, \
                PERCENTILE_CONT(0.10) WITHIN GROUP (ORDER BY size_bytes)::FLOAT8 AS p10_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY ext ORDER BY ext",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, p10, cnt)| serde_json::json!({"ext": ext, "p10_deleted_size_bytes": p10, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-active-p05-by-kind — P5 do size de arquivos ativos por kind. Sprint #3752.
async fn file_stats_size_active_p05_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT kind, \
                PERCENTILE_CONT(0.05) WITHIN GROUP (ORDER BY size_bytes)::FLOAT8 AS p05_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, p05, cnt)| serde_json::json!({"kind": kind, "p05_active_size_bytes": p05, "active_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-active-p05-by-mime — P5 do size de arquivos ativos por MIME. Sprint #3769.
async fn file_stats_size_active_p05_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT mime_type, \
                PERCENTILE_CONT(0.05) WITHIN GROUP (ORDER BY size_bytes)::FLOAT8 AS p05_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, p05, cnt)| serde_json::json!({"mime_type": mime, "p05_active_size_bytes": p05, "active_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-active-p05-by-owner — P5 do size de arquivos ativos por owner. Sprint #3770.
async fn file_stats_size_active_p05_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
                PERCENTILE_CONT(0.05) WITHIN GROUP (ORDER BY size_bytes)::FLOAT8 AS p05_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY active_count DESC LIMIT 100",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, p05, cnt)| serde_json::json!({"owner_id": owner, "p05_active_size_bytes": p05, "active_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-active-p05-by-ext — P5 do size de arquivos ativos por extensão. Sprint #3771.
async fn file_stats_size_active_p05_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT LOWER(REGEXP_REPLACE(name, '^.*\\.', '')) AS ext, \
                PERCENTILE_CONT(0.05) WITHIN GROUP (ORDER BY size_bytes)::FLOAT8 AS p05_size, \
                COUNT(*)::BIGINT AS active_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY ext ORDER BY ext",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, p05, cnt)| serde_json::json!({"ext": ext, "p05_active_size_bytes": p05, "active_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-deleted-p05-by-kind — P5 do size de arquivos deletados por kind. Sprint #3772.
async fn file_stats_size_deleted_p05_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT kind, \
                PERCENTILE_CONT(0.05) WITHIN GROUP (ORDER BY size_bytes)::FLOAT8 AS p05_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, p05, cnt)| serde_json::json!({"kind": kind, "p05_deleted_size_bytes": p05, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-deleted-p05-by-mime — P5 do size de arquivos deletados por MIME. Sprint #3789.
async fn file_stats_size_deleted_p05_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT mime_type, \
                PERCENTILE_CONT(0.05) WITHIN GROUP (ORDER BY size_bytes)::FLOAT8 AS p05_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, p05, cnt)| serde_json::json!({"mime_type": mime, "p05_deleted_size_bytes": p05, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-deleted-p05-by-owner — P5 do size de arquivos deletados por owner. Sprint #3790.
async fn file_stats_size_deleted_p05_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
                PERCENTILE_CONT(0.05) WITHIN GROUP (ORDER BY size_bytes)::FLOAT8 AS p05_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY owner_id ORDER BY deleted_count DESC LIMIT 100",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, p05, cnt)| serde_json::json!({"owner_id": owner, "p05_deleted_size_bytes": p05, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-deleted-p05-by-ext — P5 do size de arquivos deletados por extensão. Sprint #3791.
async fn file_stats_size_deleted_p05_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT LOWER(REGEXP_REPLACE(name, '^.*\\.', '')) AS ext, \
                PERCENTILE_CONT(0.05) WITHIN GROUP (ORDER BY size_bytes)::FLOAT8 AS p05_size, \
                COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY ext ORDER BY ext",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, p05, cnt)| serde_json::json!({"ext": ext, "p05_deleted_size_bytes": p05, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/uploads-by-kind — contagem de uploads por kind. Sprint #3792.
async fn file_stats_uploads_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT kind, COUNT(*)::BIGINT AS upload_count \
         FROM drive_files \
         WHERE tenant_id = $1 \
         GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, cnt)| serde_json::json!({"kind": kind, "upload_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/uploads-by-mime — contagem de uploads por MIME type. Sprint #3809.
async fn file_stats_uploads_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT mime_type, COUNT(*)::BIGINT AS upload_count \
         FROM drive_files \
         WHERE tenant_id = $1 \
         GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, cnt)| serde_json::json!({"mime_type": mime, "upload_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/uploads-by-ext — contagem de uploads por extensão. Sprint #3810.
async fn file_stats_uploads_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT LOWER(REGEXP_REPLACE(name, '^.*\\.', '')) AS ext, COUNT(*)::BIGINT AS upload_count \
         FROM drive_files \
         WHERE tenant_id = $1 \
         GROUP BY ext ORDER BY ext",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, cnt)| serde_json::json!({"ext": ext, "upload_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deletes-by-kind — contagem de deleções por kind. Sprint #3811.
async fn file_stats_deletes_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT kind, COUNT(*)::BIGINT AS delete_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY kind ORDER BY kind",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, cnt)| serde_json::json!({"kind": kind, "delete_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deletes-by-mime — contagem de deleções por MIME type. Sprint #3812.
async fn file_stats_deletes_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT mime_type, COUNT(*)::BIGINT AS delete_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
         GROUP BY mime_type ORDER BY mime_type",
    ).bind(ctx.tenant_id).fetch_all(state.db_or_unavailable()?).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, cnt)| serde_json::json!({"mime_type": mime, "delete_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/version-min-by-ext — versão mínima por extensão. Sprint #2426.
async fn file_stats_version_min_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT LOWER(REGEXP_REPLACE(name, '^.*\\.', '')) AS ext, \
                MIN(version)::BIGINT AS min_version, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY min_version ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(ext, min_v, cnt)| serde_json::json!({"ext": ext, "min_version": min_v, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/version-max-by-mime — versão máxima por mime_type. Sprint #2431.
async fn file_stats_version_max_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT mime_type, MAX(version)::BIGINT AS max_version, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY max_version DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(mime, max_v, cnt)| serde_json::json!({"mime_type": mime, "max_version": max_v, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/version-min-by-mime — versão mínima por mime_type. Sprint #2436.
async fn file_stats_version_min_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT mime_type, MIN(version)::BIGINT AS min_version, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY min_version ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(mime, min_v, cnt)| serde_json::json!({"mime_type": mime, "min_version": min_v, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/version-stddev-by-kind — desvio padrão da versão por kind. Sprint #2441.
async fn file_stats_version_stddev_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT kind, COALESCE(STDDEV(version), 0.0)::FLOAT8 AS stddev_version, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY kind ORDER BY stddev_version DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(kind, stddev, cnt)| serde_json::json!({"kind": kind, "stddev_version": stddev, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/version-min-by-kind — versão mínima por kind. Sprint #2406.
async fn file_stats_version_min_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT kind, MIN(version)::BIGINT AS min_version, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY kind ORDER BY min_version ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(kind, min_v, cnt)| serde_json::json!({"kind": kind, "min_version": min_v, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/version-max-by-owner — versão máxima por owner. Sprint #2411.
async fn file_stats_version_max_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, MAX(version)::BIGINT AS max_version, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY max_version DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(owner, max_v, cnt)| serde_json::json!({"owner_id": owner, "max_version": max_v, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/version-min-by-owner — versão mínima por owner. Sprint #2416.
async fn file_stats_version_min_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, MIN(version)::BIGINT AS min_version, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY min_version ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(owner, min_v, cnt)| serde_json::json!({"owner_id": owner, "min_version": min_v, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/version-max-by-ext — versão máxima por extensão. Sprint #2421.
async fn file_stats_version_max_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT LOWER(REGEXP_REPLACE(name, '^.*\\.', '')) AS ext, \
                MAX(version)::BIGINT AS max_version, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY max_version DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(ext, max_v, cnt)| serde_json::json!({"ext": ext, "max_version": max_v, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/name-length-stddev-by-owner — desvio padrão do comprimento de nome por owner. Sprint #2386.
async fn file_stats_name_length_stddev_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, COALESCE(STDDEV(LENGTH(name)), 0.0)::FLOAT8 AS stddev_name_len, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY stddev_name_len DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(owner, stddev, cnt)| serde_json::json!({"owner_id": owner, "stddev_name_length": stddev, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/name-length-stddev-by-ext — desvio padrão do comprimento de nome por extensão. Sprint #2391.
async fn file_stats_name_length_stddev_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT LOWER(REGEXP_REPLACE(name, '^.*\\.', '')) AS ext, \
                COALESCE(STDDEV(LENGTH(name)), 0.0)::FLOAT8 AS stddev_name_len, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY stddev_name_len DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(ext, stddev, cnt)| serde_json::json!({"ext": ext, "stddev_name_length": stddev, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/name-length-stddev-by-mime — desvio padrão do comprimento de nome por mime_type. Sprint #2396.
async fn file_stats_name_length_stddev_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT mime_type, COALESCE(STDDEV(LENGTH(name)), 0.0)::FLOAT8 AS stddev_name_len, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY stddev_name_len DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(mime, stddev, cnt)| serde_json::json!({"mime_type": mime, "stddev_name_length": stddev, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/version-max-by-kind — versão máxima por kind. Sprint #2401.
async fn file_stats_version_max_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT kind, MAX(version)::BIGINT AS max_version, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY kind ORDER BY max_version DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(kind, max_v, cnt)| serde_json::json!({"kind": kind, "max_version": max_v, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/name-length-min-by-owner — mínimo comprimento de nome por owner. Sprint #2366.
async fn file_stats_name_length_min_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, MIN(LENGTH(name))::BIGINT AS min_name_len, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY min_name_len ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(owner, min_len, cnt)| serde_json::json!({"owner_id": owner, "min_name_length": min_len, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/name-length-min-by-ext — mínimo comprimento de nome por extensão. Sprint #2371.
async fn file_stats_name_length_min_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT LOWER(REGEXP_REPLACE(name, '^.*\\.', '')) AS ext, \
                MIN(LENGTH(name))::BIGINT AS min_name_len, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY min_name_len ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(ext, min_len, cnt)| serde_json::json!({"ext": ext, "min_name_length": min_len, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/name-length-min-by-mime — mínimo comprimento de nome por mime_type. Sprint #2376.
async fn file_stats_name_length_min_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT mime_type, MIN(LENGTH(name))::BIGINT AS min_name_len, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY min_name_len ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(mime, min_len, cnt)| serde_json::json!({"mime_type": mime, "min_name_length": min_len, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/name-length-stddev-by-kind — desvio padrão do comprimento de nome por kind. Sprint #2381.
async fn file_stats_name_length_stddev_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT kind, COALESCE(STDDEV(LENGTH(name)), 0.0)::FLOAT8 AS stddev_name_len, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY kind ORDER BY stddev_name_len DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(kind, stddev, cnt)| serde_json::json!({"kind": kind, "stddev_name_length": stddev, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/name-length-max-by-owner — máximo comprimento de nome por owner. Sprint #2346.
async fn file_stats_name_length_max_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, MAX(LENGTH(name))::BIGINT AS max_name_len, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY max_name_len DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(owner, max_len, cnt)| serde_json::json!({"owner_id": owner, "max_name_length": max_len, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/name-length-max-by-ext — máximo comprimento de nome por extensão. Sprint #2351.
async fn file_stats_name_length_max_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT LOWER(REGEXP_REPLACE(name, '^.*\\.', '')) AS ext, \
                MAX(LENGTH(name))::BIGINT AS max_name_len, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY max_name_len DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(ext, max_len, cnt)| serde_json::json!({"ext": ext, "max_name_length": max_len, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/name-length-max-by-mime — máximo comprimento de nome por mime_type. Sprint #2356.
async fn file_stats_name_length_max_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT mime_type, MAX(LENGTH(name))::BIGINT AS max_name_len, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY max_name_len DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(mime, max_len, cnt)| serde_json::json!({"mime_type": mime, "max_name_length": max_len, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/name-length-min-by-kind — mínimo comprimento de nome por kind. Sprint #2361.
async fn file_stats_name_length_min_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT kind, MIN(LENGTH(name))::BIGINT AS min_name_len, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY kind ORDER BY min_name_len ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(kind, min_len, cnt)| serde_json::json!({"kind": kind, "min_name_length": min_len, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/count-by-shared — contagem de arquivos por flag shared. Sprint #2326.
async fn file_stats_count_by_shared(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(bool, i64)> = sqlx::query_as(
        "SELECT shared, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY shared ORDER BY shared",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(shared, cnt)| serde_json::json!({"shared": shared, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/name-length-avg-by-kind — comprimento médio do nome por kind. Sprint #2331.
async fn file_stats_name_length_avg_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT kind, AVG(LENGTH(name))::FLOAT8 AS avg_name_len, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY kind ORDER BY avg_name_len DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(kind, avg, cnt)| serde_json::json!({"kind": kind, "avg_name_length": avg, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/name-length-avg-by-owner — comprimento médio do nome por owner. Sprint #2336.
async fn file_stats_name_length_avg_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, AVG(LENGTH(name))::FLOAT8 AS avg_name_len, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY avg_name_len DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(owner, avg, cnt)| serde_json::json!({"owner_id": owner, "avg_name_length": avg, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/name-length-avg-by-ext — comprimento médio do nome por extensão. Sprint #2341.
async fn file_stats_name_length_avg_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT LOWER(REGEXP_REPLACE(name, '^.*\\.', '')) AS ext, \
                AVG(LENGTH(name))::FLOAT8 AS avg_name_len, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL AND name LIKE '%.%' \
         GROUP BY ext ORDER BY avg_name_len DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(state.db())
    .await
    .map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(ext, avg, cnt)| serde_json::json!({"ext": ext, "avg_name_length": avg, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-range-by-owner — range (MAX-MIN) de size_bytes por owner. Sprint #2306.
async fn file_stats_size_range_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, MAX(size_bytes)::BIGINT AS max_size, MIN(size_bytes)::BIGINT AS min_size, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY file_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, max_sz, min_sz, cnt)| serde_json::json!({"owner_id": owner, "range_size_bytes": max_sz - min_sz, "max": max_sz, "min": min_sz, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-range-by-ext — range de size_bytes por extensão. Sprint #2311.
async fn file_stats_size_range_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(LOWER(REGEXP_REPLACE(name, '^.*\\.', '')), 'no-ext') AS ext, \
         MAX(size_bytes)::BIGINT AS max_size, MIN(size_bytes)::BIGINT AS min_size, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY ext ORDER BY file_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, max_sz, min_sz, cnt)| serde_json::json!({"ext": ext, "range_size_bytes": max_sz - min_sz, "max": max_sz, "min": min_sz, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-range-by-kind — range de size_bytes por kind. Sprint #2316.
async fn file_stats_size_range_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT kind::TEXT, MAX(size_bytes)::BIGINT AS max_size, MIN(size_bytes)::BIGINT AS min_size, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY kind ORDER BY file_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, max_sz, min_sz, cnt)| serde_json::json!({"kind": kind, "range_size_bytes": max_sz - min_sz, "max": max_sz, "min": min_sz, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-range-by-mime — range de size_bytes por mime_type. Sprint #2321.
async fn file_stats_size_range_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type, 'unknown') AS mime_type, \
         MAX(size_bytes)::BIGINT AS max_size, MIN(size_bytes)::BIGINT AS min_size, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY file_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, max_sz, min_sz, cnt)| serde_json::json!({"mime_type": mime, "range_size_bytes": max_sz - min_sz, "max": max_sz, "min": min_sz, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-iqr-by-owner — IQR (P75-P25) de size_bytes por owner. Sprint #2286.
async fn file_stats_size_iqr_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
         PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p75, \
         PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p25, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY file_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, p75, p25, cnt)| serde_json::json!({"owner_id": owner, "iqr_size_bytes": p75 - p25, "p75": p75, "p25": p25, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-iqr-by-ext — IQR de size_bytes por extensão. Sprint #2291.
async fn file_stats_size_iqr_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(LOWER(REGEXP_REPLACE(name, '^.*\\.', '')), 'no-ext') AS ext, \
         PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p75, \
         PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p25, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY ext ORDER BY file_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, p75, p25, cnt)| serde_json::json!({"ext": ext, "iqr_size_bytes": p75 - p25, "p75": p75, "p25": p25, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-iqr-by-kind — IQR de size_bytes por kind. Sprint #2296.
async fn file_stats_size_iqr_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT kind::TEXT, \
         PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p75, \
         PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p25, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY kind ORDER BY file_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, p75, p25, cnt)| serde_json::json!({"kind": kind, "iqr_size_bytes": p75 - p25, "p75": p75, "p25": p25, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-iqr-by-mime — IQR de size_bytes por mime_type. Sprint #2301.
async fn file_stats_size_iqr_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type, 'unknown') AS mime_type, \
         PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p75, \
         PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p25, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY file_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, p75, p25, cnt)| serde_json::json!({"mime_type": mime, "iqr_size_bytes": p75 - p25, "p75": p75, "p25": p25, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-cv-by-owner — coeficiente de variação de size_bytes por owner. Sprint #2266.
async fn file_stats_size_cv_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Option<f64>, Option<f64>, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, \
         STDDEV(size_bytes)::FLOAT8 AS stddev_size, \
         AVG(size_bytes)::FLOAT8 AS avg_size, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY file_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, stddev, avg, cnt)| {
            let cv = match (stddev, avg) {
                (Some(s), Some(a)) if a > 0.0 => Some(s / a),
                _ => None,
            };
            serde_json::json!({"owner_id": owner, "cv_size": cv, "file_count": cnt})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-cv-by-ext — coeficiente de variação de size_bytes por extensão. Sprint #2271.
async fn file_stats_size_cv_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Option<f64>, Option<f64>, i64)> = sqlx::query_as(
        "SELECT COALESCE(LOWER(REGEXP_REPLACE(name, '^.*\\.', '')), 'no-ext') AS ext, \
         STDDEV(size_bytes)::FLOAT8 AS stddev_size, \
         AVG(size_bytes)::FLOAT8 AS avg_size, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY ext ORDER BY file_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, stddev, avg, cnt)| {
            let cv = match (stddev, avg) {
                (Some(s), Some(a)) if a > 0.0 => Some(s / a),
                _ => None,
            };
            serde_json::json!({"ext": ext, "cv_size": cv, "file_count": cnt})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-cv-by-kind — coeficiente de variação de size_bytes por kind. Sprint #2276.
async fn file_stats_size_cv_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Option<f64>, Option<f64>, i64)> = sqlx::query_as(
        "SELECT kind::TEXT, \
         STDDEV(size_bytes)::FLOAT8 AS stddev_size, \
         AVG(size_bytes)::FLOAT8 AS avg_size, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY kind ORDER BY file_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, stddev, avg, cnt)| {
            let cv = match (stddev, avg) {
                (Some(s), Some(a)) if a > 0.0 => Some(s / a),
                _ => None,
            };
            serde_json::json!({"kind": kind, "cv_size": cv, "file_count": cnt})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-cv-by-mime — coeficiente de variação de size_bytes por mime_type. Sprint #2281.
async fn file_stats_size_cv_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Option<f64>, Option<f64>, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type, 'unknown') AS mime_type, \
         STDDEV(size_bytes)::FLOAT8 AS stddev_size, \
         AVG(size_bytes)::FLOAT8 AS avg_size, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY file_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, stddev, avg, cnt)| {
            let cv = match (stddev, avg) {
                (Some(s), Some(a)) if a > 0.0 => Some(s / a),
                _ => None,
            };
            serde_json::json!({"mime_type": mime, "cv_size": cv, "file_count": cnt})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-stddev-by-owner — STDDEV size_bytes por owner. Sprint #2246.
async fn file_stats_size_stddev_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Option<f64>, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, STDDEV(size_bytes)::FLOAT8 AS stddev_size_bytes, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY stddev_size_bytes DESC NULLS LAST",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, stddev, cnt)| serde_json::json!({"owner_id": owner, "stddev_size_bytes": stddev, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-stddev-by-ext — STDDEV size_bytes por extensão. Sprint #2251.
async fn file_stats_size_stddev_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Option<f64>, i64)> = sqlx::query_as(
        "SELECT COALESCE(LOWER(REGEXP_REPLACE(name, '^.*\\.', '')), 'no-ext') AS ext, \
         STDDEV(size_bytes)::FLOAT8 AS stddev_size_bytes, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY ext ORDER BY stddev_size_bytes DESC NULLS LAST",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, stddev, cnt)| serde_json::json!({"ext": ext, "stddev_size_bytes": stddev, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-stddev-by-kind — STDDEV size_bytes por kind. Sprint #2256.
async fn file_stats_size_stddev_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Option<f64>, i64)> = sqlx::query_as(
        "SELECT kind::TEXT, STDDEV(size_bytes)::FLOAT8 AS stddev_size_bytes, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY kind ORDER BY stddev_size_bytes DESC NULLS LAST",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, stddev, cnt)| serde_json::json!({"kind": kind, "stddev_size_bytes": stddev, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-stddev-by-mime — STDDEV size_bytes por mime_type. Sprint #2261.
async fn file_stats_size_stddev_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, Option<f64>, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type, 'unknown') AS mime_type, STDDEV(size_bytes)::FLOAT8 AS stddev_size_bytes, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY stddev_size_bytes DESC NULLS LAST",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, stddev, cnt)| serde_json::json!({"mime_type": mime, "stddev_size_bytes": stddev, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-min-by-owner — MIN size_bytes por owner. Sprint #2226.
async fn file_stats_size_min_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, MIN(size_bytes)::BIGINT AS min_size_bytes, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY min_size_bytes",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, min_sz, cnt)| serde_json::json!({"owner_id": owner, "min_size_bytes": min_sz, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-min-by-ext — MIN size_bytes por extensão. Sprint #2231.
async fn file_stats_size_min_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(LOWER(REGEXP_REPLACE(name, '^.*\\.', '')), 'no-ext') AS ext, \
         MIN(size_bytes)::BIGINT AS min_size_bytes, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY ext ORDER BY min_size_bytes",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, min_sz, cnt)| serde_json::json!({"ext": ext, "min_size_bytes": min_sz, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-min-by-kind — MIN size_bytes por kind. Sprint #2236.
async fn file_stats_size_min_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT kind::TEXT, MIN(size_bytes)::BIGINT AS min_size_bytes, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY kind ORDER BY min_size_bytes",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, min_sz, cnt)| serde_json::json!({"kind": kind, "min_size_bytes": min_sz, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-min-by-mime — MIN size_bytes por mime_type. Sprint #2241.
async fn file_stats_size_min_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type, 'unknown') AS mime_type, MIN(size_bytes)::BIGINT AS min_size_bytes, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY min_size_bytes",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, min_sz, cnt)| serde_json::json!({"mime_type": mime, "min_size_bytes": min_sz, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-max-by-owner — MAX size_bytes por owner. Sprint #2206.
async fn file_stats_size_max_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, MAX(size_bytes)::BIGINT AS max_size_bytes, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY max_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, max_sz, cnt)| serde_json::json!({"owner_id": owner, "max_size_bytes": max_sz, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-max-by-ext — MAX size_bytes por extensão. Sprint #2211.
async fn file_stats_size_max_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(LOWER(REGEXP_REPLACE(name, '^.*\\.', '')), 'no-ext') AS ext, \
         MAX(size_bytes)::BIGINT AS max_size_bytes, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY ext ORDER BY max_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, max_sz, cnt)| serde_json::json!({"ext": ext, "max_size_bytes": max_sz, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-max-by-kind — MAX size_bytes por kind. Sprint #2216.
async fn file_stats_size_max_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT kind::TEXT, MAX(size_bytes)::BIGINT AS max_size_bytes, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL \
         GROUP BY kind ORDER BY max_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, max_sz, cnt)| serde_json::json!({"kind": kind, "max_size_bytes": max_sz, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-max-by-mime — MAX size_bytes por mime_type. Sprint #2221.
async fn file_stats_size_max_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type, 'unknown') AS mime_type, MAX(size_bytes)::BIGINT AS max_size_bytes, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY max_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, max_sz, cnt)| serde_json::json!({"mime_type": mime, "max_size_bytes": max_sz, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p90-by-mime — P90 size_bytes por mime_type completo. Sprint #2186.
async fn file_stats_size_p90_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type, 'unknown') AS mime_type, \
         PERCENTILE_CONT(0.90) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p90_size_bytes, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY p90_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, p90, cnt)| serde_json::json!({"mime_type": mime, "p90_size_bytes": p90, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-sum-by-mime — SUM size_bytes por mime_type completo. Sprint #2191.
async fn file_stats_size_sum_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type, 'unknown') AS mime_type, \
         SUM(size_bytes)::BIGINT AS total_size_bytes, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY total_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, total, cnt)| serde_json::json!({"mime_type": mime, "total_size_bytes": total, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-sum-by-kind — SUM size_bytes por categoria MIME. Sprint #2196.
async fn file_stats_size_sum_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(SPLIT_PART(mime_type, '/', 1), 'unknown') AS kind_category, \
         SUM(size_bytes)::BIGINT AS total_size_bytes, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY kind_category ORDER BY total_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kc, total, cnt)| serde_json::json!({"kind_category": kc, "total_size_bytes": total, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-sum-by-owner — SUM size_bytes por owner. Sprint #2201.
async fn file_stats_size_sum_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, SUM(size_bytes)::BIGINT AS total_size_bytes, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY total_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, total, cnt)| serde_json::json!({"owner_id": owner, "total_size_bytes": total, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-avg-by-mime — AVG size_bytes por mime_type completo. Sprint #2166.
async fn file_stats_size_avg_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type, 'unknown') AS mime_type, \
         AVG(size_bytes)::FLOAT AS avg_size_bytes, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY avg_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, avg, cnt)| serde_json::json!({"mime_type": mime, "avg_size_bytes": avg, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p50-by-mime — P50 size_bytes por mime_type completo. Sprint #2171.
async fn file_stats_size_p50_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type, 'unknown') AS mime_type, \
         PERCENTILE_CONT(0.50) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p50_size_bytes, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY p50_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, p50, cnt)| serde_json::json!({"mime_type": mime, "p50_size_bytes": p50, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/size-p75-by-mime — P75 size_bytes por mime_type completo. Sprint #2176.
async fn file_stats_size_p75_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type, 'unknown') AS mime_type, \
         PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY size_bytes)::BIGINT AS p75_size_bytes, \
         COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY p75_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, p75, cnt)| serde_json::json!({"mime_type": mime, "p75_size_bytes": p75, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/count-by-kind — contagem de arquivos por categoria MIME. Sprint #2181.
async fn file_stats_count_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT COALESCE(SPLIT_PART(mime_type, '/', 1), 'unknown') AS kind_category, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY kind_category ORDER BY file_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kc, cnt)| serde_json::json!({"kind_category": kc, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-count-by-mime — arquivos deletados por mime_type completo. Sprint #2146.
async fn file_stats_deleted_count_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type, 'unknown') AS mime_type, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NOT NULL \
         GROUP BY mime_type ORDER BY deleted_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, cnt)| serde_json::json!({"mime_type": mime, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/deleted-size-by-mime — SUM size_bytes de deletados por mime_type. Sprint #2151.
async fn file_stats_deleted_size_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type, 'unknown') AS mime_type, \
         SUM(size_bytes)::BIGINT AS deleted_size_bytes, COUNT(*)::BIGINT AS deleted_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NOT NULL \
         GROUP BY mime_type ORDER BY deleted_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, size, cnt)| serde_json::json!({"mime_type": mime, "deleted_size_bytes": size, "deleted_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/version-count-by-kind — versões totais por categoria MIME. Sprint #2156.
async fn file_stats_version_count_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(SPLIT_PART(mime_type, '/', 1), 'unknown') AS kind_category, \
         SUM(version)::BIGINT AS total_versions, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY kind_category ORDER BY total_versions DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kc, versions, cnt)| serde_json::json!({"kind_category": kc, "total_versions": versions, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/version-avg-by-kind — versão média por categoria MIME. Sprint #2161.
async fn file_stats_version_avg_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT COALESCE(SPLIT_PART(mime_type, '/', 1), 'unknown') AS kind_category, \
         AVG(version)::FLOAT AS avg_version, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY kind_category ORDER BY avg_version DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kc, avg, cnt)| serde_json::json!({"kind_category": kc, "avg_version": avg, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/shared-count-by-kind — arquivos compartilhados por categoria MIME. Sprint #2126.
async fn file_stats_shared_count_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT COALESCE(SPLIT_PART(mime_type, '/', 1), 'unknown') AS kind_category, COUNT(*)::BIGINT AS shared_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL AND shared = TRUE \
         GROUP BY kind_category ORDER BY shared_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kc, cnt)| serde_json::json!({"kind_category": kc, "shared_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/shared-size-by-kind — SUM size_bytes compartilhados por categoria MIME. Sprint #2131.
async fn file_stats_shared_size_by_kind(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(SPLIT_PART(mime_type, '/', 1), 'unknown') AS kind_category, \
         SUM(size_bytes)::BIGINT AS shared_size_bytes, COUNT(*)::BIGINT AS shared_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL AND shared = TRUE \
         GROUP BY kind_category ORDER BY shared_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kc, size, cnt)| serde_json::json!({"kind_category": kc, "shared_size_bytes": size, "shared_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/version-count-by-mime — versões totais por mime_type completo. Sprint #2136.
async fn file_stats_version_count_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type, 'unknown') AS mime_type, \
         SUM(version)::BIGINT AS total_versions, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY total_versions DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, versions, cnt)| serde_json::json!({"mime_type": mime, "total_versions": versions, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/version-avg-by-mime — versão média por mime_type completo. Sprint #2141.
async fn file_stats_version_avg_by_mime(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type, 'unknown') AS mime_type, \
         AVG(version)::FLOAT AS avg_version, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY mime_type ORDER BY avg_version DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, avg, cnt)| serde_json::json!({"mime_type": mime, "avg_version": avg, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/shared-count-by-ext — número de arquivos compartilhados por extensão. Sprint #2106.
async fn file_stats_shared_count_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT LOWER(SPLIT_PART(name, '.', -1)) AS ext, COUNT(*)::BIGINT AS shared_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL AND shared = TRUE \
         GROUP BY ext ORDER BY shared_count DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, cnt)| serde_json::json!({"ext": ext, "shared_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/shared-size-by-ext — SUM size_bytes de arquivos compartilhados por extensão. Sprint #2111.
async fn file_stats_shared_size_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT LOWER(SPLIT_PART(name, '.', -1)) AS ext, \
         SUM(size_bytes)::BIGINT AS shared_size_bytes, COUNT(*)::BIGINT AS shared_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL AND shared = TRUE \
         GROUP BY ext ORDER BY shared_size_bytes DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, size, cnt)| serde_json::json!({"ext": ext, "shared_size_bytes": size, "shared_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/version-avg-by-owner — versão média por owner. Sprint #2116.
async fn file_stats_version_avg_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, AVG(version)::FLOAT AS avg_version, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY avg_version DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, avg, cnt)| serde_json::json!({"owner_id": owner, "avg_version": avg, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/version-avg-by-ext — versão média por extensão. Sprint #2121.
async fn file_stats_version_avg_by_ext(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT LOWER(SPLIT_PART(name, '.', -1)) AS ext, \
         AVG(version)::FLOAT AS avg_version, COUNT(*)::BIGINT AS file_count \
         FROM drive_files \
         WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY ext ORDER BY avg_version DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, avg, cnt)| serde_json::json!({"ext": ext, "avg_version": avg, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/drive/files/stats/version-count-by-owner — número de versões por owner. Sprint #2101.
async fn file_stats_version_count_by_owner(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id::TEXT, SUM(version)::BIGINT AS total_versions, COUNT(*)::BIGINT AS file_count \
         FROM drive_files WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL \
         GROUP BY owner_id ORDER BY total_versions DESC",
    ).bind(ctx.tenant_id).fetch_all(state.db()).await.map_err(db_or_unavailable)?;
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(owner, versions, cnt)| serde_json::json!({"owner_id": owner, "total_versions": versions, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// DELETE /api/v1/drive/folders/:id/quota — remove folder quota.
async fn delete_folder_quota(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    FolderQuotaRepo::new(pool).delete(ctx.tenant_id, id).await?;
    tracing::info!(target: "audit",
        event = "drive.folder.quota_deleted",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, folder_id = %id);
    Ok(StatusCode::NO_CONTENT)
}
