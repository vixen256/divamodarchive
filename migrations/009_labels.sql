CREATE TABLE reservation_labels (
	user_id bigint not null references users on delete cascade,
	reservation_type int not null default 0,
	id int not null,
	label text not null
);
