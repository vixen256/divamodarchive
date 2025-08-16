ALTER TABLE posts ADD explicit bool not null DEFAULT false;
ALTER TABLE posts ADD explicit_reason text;
ALTER TABLE users ADD show_explicit bool not null DEFAULT false;
