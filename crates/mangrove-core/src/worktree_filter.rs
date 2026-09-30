//! Clean already-pinned bytes in memory. No blob writes, file reads or custom
//! processes: libgit2 supplies its built-in CRLF and ident filters here.
use crate::Result;
use libgit2_sys as raw;
use std::{
    ffi::{c_char, c_void, CStr, CString},
    ptr,
};

// The filter APIs are public libgit2 APIs missing from libgit2-sys's bindings.
unsafe extern "C" {
    fn git_filter_list_load(
        out: *mut *mut c_void,
        repo: *mut raw::git_repository,
        blob: *mut raw::git_blob,
        path: *const c_char,
        mode: i32,
        flags: u32,
    ) -> i32;
    fn git_filter_list_apply_to_buffer(
        out: *mut raw::git_buf,
        filters: *mut c_void,
        input: *const c_char,
        len: usize,
    ) -> i32;
    fn git_filter_list_free(filters: *mut c_void);
}
struct Filter {
    repository: *mut raw::git_repository,
    filters: *mut c_void,
    output: raw::git_buf,
}
impl Drop for Filter {
    fn drop(&mut self) {
        // Each pointer is null or was acquired by the matching libgit2 API;
        // output and filters are released before the repository they borrow.
        unsafe {
            raw::git_buf_dispose(&mut self.output);
            git_filter_list_free(self.filters);
            raw::git_repository_free(self.repository);
        }
    }
}
fn check(code: i32) -> Result<()> {
    if code >= 0 {
        return Ok(());
    }
    // libgit2 keeps its last error alive on this thread until the next call.
    unsafe {
        let error = raw::git_error_last();
        if !error.is_null() && !(*error).message.is_null() {
            return Err(CStr::from_ptr((*error).message)
                .to_string_lossy()
                .into_owned());
        }
    }
    Err("Impossible d'appliquer les filtres Git en mémoire.".into())
}
pub(crate) fn clean(repository: &git2::Repository, path: &[u8], bytes: &[u8]) -> Result<Vec<u8>> {
    let location = repository.workdir().unwrap_or(repository.path());
    let location =
        CString::new(location.as_os_str().as_encoded_bytes()).map_err(|e| e.to_string())?;
    let path = CString::new(path).map_err(|e| e.to_string())?;
    let mut filter = Filter {
        repository: ptr::null_mut(),
        filters: ptr::null_mut(),
        output: raw::git_buf {
            ptr: ptr::null_mut(),
            reserved: 0,
            size: 0,
        },
    };
    // Repository is already initialized by git2. NO_SEARCH keeps the handle on
    // the selected worktree; FROM_ENV is deliberately absent. Load and apply
    // never open the target file: the input came from its nofollow descriptor.
    unsafe {
        check(raw::git_repository_open_ext(
            &mut filter.repository,
            location.as_ptr(),
            git2::RepositoryOpenFlags::NO_SEARCH.bits(),
            ptr::null(),
        ))?;
        check(git_filter_list_load(
            &mut filter.filters,
            filter.repository,
            ptr::null_mut(),
            path.as_ptr(),
            1, /* GIT_FILTER_TO_ODB */
            0,
        ))?;
        check(git_filter_list_apply_to_buffer(
            &mut filter.output,
            filter.filters,
            bytes.as_ptr().cast(),
            bytes.len(),
        ))?;
        if filter.output.size == 0 {
            return Ok(Vec::new());
        }
        // Successful apply owns a size-byte allocation until Filter::drop.
        Ok(std::slice::from_raw_parts(filter.output.ptr.cast::<u8>(), filter.output.size).to_vec())
    }
}
