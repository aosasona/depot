ALTER TABLE service_configs
ADD COLUMN persistent_volumes_json TEXT NOT NULL DEFAULT '[]';
