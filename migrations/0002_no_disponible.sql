-- Marca de video NO reproducible: premiere futura, privado, solo-miembros,
-- geo-bloqueado, borrado, o directo terminado sin formato descargable. Se pone
-- al fallar la reproducción (el /stream lo detecta al no recibir bytes de
-- yt-dlp). El feed oculta estos, y además los de fecha FUTURA (premieres que aún
-- no se emiten), que se filtran por `published`.
ALTER TABLE videos ADD COLUMN no_disponible INTEGER NOT NULL DEFAULT 0;
