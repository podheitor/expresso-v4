//! Development bootstrap for first-run local environments.

use uuid::Uuid;

use crate::state::AppState;

pub async fn ensure_dev_bootstrap(state: &AppState) -> anyhow::Result<()> {
    let mut tx = state.db().begin().await?;

    let tenant_id: Uuid =
        match sqlx::query_scalar("SELECT id FROM tenants ORDER BY created_at ASC LIMIT 1")
            .fetch_optional(&mut *tx)
            .await?
        {
            Some(id) => id,
            None => {
                sqlx::query_scalar(
                    r#"
                INSERT INTO tenants (slug, name, plan, status)
                VALUES ($1, $2, 'standard', 'active')
                RETURNING id
                "#,
                )
                .bind("default")
                .bind("Expresso Default Tenant")
                .fetch_one(&mut *tx)
                .await?
            }
        };

    let admin_email = format!("admin@{}", state.cfg().mail_server.domain);

    let user_id: Uuid = match sqlx::query_scalar(
        r#"
        SELECT id FROM users
        WHERE tenant_id = $1
        ORDER BY created_at ASC
        LIMIT 1
        "#,
    )
    .bind(tenant_id)
    .fetch_optional(&mut *tx)
    .await?
    {
        Some(id) => id,
        None => {
            sqlx::query_scalar(
                r#"
                INSERT INTO users (tenant_id, email, display_name, role, is_active)
                VALUES ($1, $2, $3, 'tenant_admin', true)
                RETURNING id
                "#,
            )
            .bind(tenant_id)
            .bind(&admin_email)
            .bind("Administrador")
            .fetch_one(&mut *tx)
            .await?
        }
    };

    let folders: [(&str, Option<&str>); 5] = [
        ("INBOX", Some(r"\Inbox")),
        ("Sent", Some(r"\Sent")),
        ("Drafts", Some(r"\Drafts")),
        ("Trash", Some(r"\Trash")),
        ("Junk", Some(r"\Junk")),
    ];

    for (folder_name, special_use) in folders {
        sqlx::query(
            r#"
            INSERT INTO mailboxes (user_id, tenant_id, folder_name, special_use, subscribed)
            VALUES ($1, $2, $3, $4, true)
            ON CONFLICT (user_id, folder_name) DO NOTHING
            "#,
        )
        .bind(user_id)
        .bind(tenant_id)
        .bind(folder_name)
        .bind(special_use)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;

    tracing::info!(%tenant_id, %user_id, %admin_email, "mail bootstrap ensured");
    Ok(())
}
