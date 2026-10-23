-- Coin icon library: icons added from the console (upload, link, CoinGecko). Files dropped into
-- the controller's icon directory are read from disk and are not stored here.
CREATE TABLE coin_icons(
    symbol text PRIMARY KEY CHECK (symbol ~ '^[A-Z0-9._-]{1,20}$'),
    content_type text NOT NULL CHECK (content_type IN ('image/svg+xml','image/png','image/webp','image/jpeg')),
    data bytea NOT NULL CHECK (octet_length(data) BETWEEN 1 AND 262144),
    source text NOT NULL CHECK (source IN ('upload','url','coingecko')),
    source_ref text,
    updated_at timestamptz NOT NULL DEFAULT now()
);
