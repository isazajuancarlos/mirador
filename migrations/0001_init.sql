-- mirador — reproductor de YouTube: SOLO reproducción + suscripciones.
-- Una base por instancia (estándar, directiva 26). Todo local.

-- Canales suscritos. `channel_id` es el UC... de YouTube (identidad estable);
-- el `title` se cachea del feed para no depender de la red al listar.
CREATE TABLE subscriptions (
    channel_id  TEXT PRIMARY KEY,
    title       TEXT NOT NULL,
    added_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- Videos vistos en el feed RSS de cada suscripción. Es CACHÉ: se repuebla al
-- actualizar, y se borra en cascada si se quita el canal. No guarda el stream
-- (eso lo resuelve yt-dlp al reproducir, y sus URLs caducan).
CREATE TABLE videos (
    video_id    TEXT PRIMARY KEY,
    channel_id  TEXT NOT NULL REFERENCES subscriptions(channel_id) ON DELETE CASCADE,
    title       TEXT NOT NULL,
    published   TIMESTAMP NOT NULL,
    thumbnail   TEXT,
    fetched_at  TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- El feed se ordena por fecha de publicación descendente (lo más nuevo primero).
CREATE INDEX idx_videos_published ON videos (published DESC);
