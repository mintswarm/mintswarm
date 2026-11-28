CREATE TABLE projects (
    project_id text PRIMARY KEY,
    repo text NOT NULL,
    host text NOT NULL,
    installation_id bigint NOT NULL,
    weight integer NOT NULL CHECK (weight > 0),
    row_version bigint NOT NULL
);
