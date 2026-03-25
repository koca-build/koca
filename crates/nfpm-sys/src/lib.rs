use std::ffi::CString;

mod cgo {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

#[derive(Clone, Debug)]
pub enum NfpmError {
    JSON,
    OutputFile,
    PkgCreation,
}

pub fn run_bundle(output_file: &str, format: &str, input_json: &str) -> Result<(), NfpmError> {
    let raw_output_file =
        CString::new(output_file).expect("output file should be CString compatible");
    let raw_format = CString::new(format).expect("input JSON should be CString compatible");
    let raw_input_json = CString::new(input_json).expect("input JSON should be CString compatible");

    let resp_status = unsafe {
        cgo::runBundle(
            raw_output_file.as_ptr() as *mut i8,
            raw_format.as_ptr() as *mut i8,
            raw_input_json.as_ptr() as *mut i8,
        )
    };

    match resp_status {
        cgo::STATUS_SUCCESS => Ok(()),
        cgo::STATUS_JSON => Err(NfpmError::JSON),
        cgo::STATUS_OUTPUT_FILE => Err(NfpmError::OutputFile),
        cgo::STATUS_PKG_CREATION => Err(NfpmError::PkgCreation),
        _ => panic!("all possible cases should have been covered"),
    }
}
