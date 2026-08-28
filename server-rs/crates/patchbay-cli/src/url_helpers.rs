use url::form_urlencoded;

pub(super) fn encoded_path_segment(value: &str) -> String {
    form_urlencoded::byte_serialize(value.as_bytes()).collect()
}
