use docs_rs_headers::{Header, testing::test_typed_encode};
use reqwest::StatusCode;

pub trait MockExt {
    fn with_typed_header<H: Header>(self, header: H) -> Self;
    fn match_typed_header<H: Header>(self, header: H) -> Self;
    fn with_status_code(self, status_code: StatusCode) -> Self;
}

impl MockExt for mockito::Mock {
    fn match_typed_header<H: Header>(self, header: H) -> Self {
        let name = H::name();
        let value = test_typed_encode(header);

        self.match_header(name, value.to_str().unwrap())
    }

    fn with_typed_header<H: Header>(self, header: H) -> Self {
        let name = H::name();
        let value = test_typed_encode(header);

        self.with_header(name, value.to_str().unwrap())
    }

    fn with_status_code(self, status_code: StatusCode) -> Self {
        self.with_status(status_code.as_u16().into())
    }
}
