use anyhow::Result;
use axum::{
    extract::{Path, State},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use http::{header::LOCATION, HeaderMap, StatusCode};
use nanoid::nanoid;
use serde::{Deserialize, Serialize};
use sqlx::{migrate::MigrateDatabase, FromRow, Sqlite, SqlitePool};
use tokio::net::TcpListener;
use tracing::{info, level_filters::LevelFilter, warn};
use tracing_subscriber::{fmt::Layer, layer::SubscriberExt, util::SubscriberInitExt, Layer as _};

const LISTEN_ADDR: &str = "127.0.0.1:8080";

#[derive(Debug, Clone)]
struct AppState {
    db: SqlitePool,
}

#[derive(Debug, Clone, Deserialize)]
struct ShortenReq {
    url: String,
}

#[derive(Debug, Serialize)]
struct ShortenRes {
    url: String,
}

#[derive(Debug, FromRow)]
struct UrlRecord {
    #[sqlx(default)]
    id: String,
    #[sqlx(default)]
    url: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let layer = Layer::new().with_filter(LevelFilter::INFO);
    tracing_subscriber::registry().with(layer).init();

    // 初始化数据库连接池
    let db_url = "sqlite://shorturl.db";
    // 初始化应用状态
    let app_state = AppState::try_new(db_url).await?;
    info!("connect to database successfully");

    let listener = TcpListener::bind(LISTEN_ADDR).await?;
    info!("listening on {}", LISTEN_ADDR);

    let app = Router::new()
        .route("/:id", get(redirect))
        .route("/", post(shorten))
        .with_state(app_state);

    axum::serve(listener, app.into_make_service()).await?;

    Ok(())
}

async fn redirect(
    Path(id): Path<String>,
    State(app_state): State<AppState>,
) -> Result<impl IntoResponse, StatusCode> {
    let url = app_state
        .get_url(&id)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let mut headers = HeaderMap::new();
    headers.insert(LOCATION, url.parse().unwrap());
    Ok((StatusCode::PERMANENT_REDIRECT, headers))
}

async fn shorten(
    State(app_state): State<AppState>,
    Json(req): Json<ShortenReq>,
) -> Result<impl IntoResponse, StatusCode> {
    let id = app_state.shorten(&req.url).await.map_err(|e| {
        warn!("Failed to shorten URL: {e}");
        StatusCode::UNPROCESSABLE_ENTITY
    })?;
    let body = Json(ShortenRes {
        url: format!("http://{}/{}", LISTEN_ADDR, id),
    });
    Ok((StatusCode::CREATED, body))
}

impl AppState {
    async fn try_new(url: &str) -> Result<Self> {
        // 如果数据库不存在，创建一个新的数据库
        if !Sqlite::database_exists(url).await? {
            Sqlite::create_database(url).await?;
        }

        let pool = SqlitePool::connect(url).await?;
        // 初始化数据库表
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS short_urls (
                id TEXT PRIMARY KEY,
                url TEXT NOT NULL UNIQUE
            )",
        )
        .execute(&pool)
        .await?;
        info!("create schema successfully");

        Ok(Self { db: pool })
    }

    /// id保存到数据库，并返回id
    async fn shorten(&self, url: &str) -> Result<String> {
        let id = nanoid!(6);
        let row:UrlRecord= sqlx::query_as("INSERT INTO short_urls (id, url) VALUES ($1, $2) ON CONFLICT(url) DO UPDATE SET url=EXCLUDED.url RETURNING id")
            .bind(&id)
            .bind(url)
            .fetch_one(&self.db)
            .await?;
        Ok(row.id)
    }

    /// 根据id查询url
    async fn get_url(&self, id: &str) -> Result<String> {
        let ret: UrlRecord = sqlx::query_as("SELECT url FROM short_urls WHERE id = $1")
            .bind(id)
            .fetch_one(&self.db)
            .await?;
        Ok(ret.url)
    }
}
