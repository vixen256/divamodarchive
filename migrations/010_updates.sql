ALTER TABLE posts ADD private bool NOT NULL DEFAULT false;

CREATE TABLE pending_uploads (
	files text[] NOT NULL,
	completed bigint[] NOT NULL,
	length bigint[] NOT NULL,
	post_id int NOT NULL REFERENCES posts ON DELETE CASCADE,
	user_id bigint NOT NULL REFERENCES users ON DELETE CASCADE
);
