use prost::Message;
use rpc::users::{ListUsersResponse, User};

#[test]
fn user_timestamps_and_pagination_round_trip() {
    let user = User {
        id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into(), display_name: "Example User".into(),
        created_at: Some(prost_types::Timestamp { seconds: 1234, nanos: 567_000 }),
        updated_at: Some(prost_types::Timestamp { seconds: 1235, nanos: 0 }),
    };
    let page = ListUsersResponse { users: vec![user.clone()], next_after_id: user.id };
    assert_eq!(ListUsersResponse::decode(page.encode_to_vec().as_slice()).unwrap(), page);
}
