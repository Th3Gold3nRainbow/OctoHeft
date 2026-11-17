use std::path::{Path, PathBuf};

#[cfg(target_os = "windows")]
mod win_ole {
    use super::validate_drag_file_path;
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::ptr::{copy_nonoverlapping, null, null_mut};
    use std::sync::atomic::{AtomicU32, Ordering};
    use windows_sys::core::{GUID, HRESULT};
    use windows_sys::Win32::Foundation::{
        GlobalFree, BOOL, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS,
        DV_E_FORMATETC, E_NOINTERFACE, E_NOTIMPL, E_POINTER, HGLOBAL, HWND, POINT, S_FALSE, S_OK,
    };
    use windows_sys::Win32::System::Com::{
        DVASPECT_CONTENT, FORMATETC, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL,
    };
    use windows_sys::Win32::System::Memory::{
        GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE,
    };
    use windows_sys::Win32::System::Ole::{
        DoDragDrop, OleInitialize, OleUninitialize, CF_HDROP, DROPEFFECT, DROPEFFECT_COPY,
        DROPEFFECT_LINK,
    };
    use windows_sys::Win32::System::SystemServices::MK_LBUTTON;
    use windows_sys::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows_sys::Win32::UI::Shell::{
        ILClone, ILCreateFromPathW, ILFindLastID, ILFree, ILRemoveLastID, SHCreateDataObject,
        SHCreateStdEnumFmtEtc, SHDoDragDrop, DROPFILES,
    };

    const IID_IUNKNOWN: GUID = GUID::from_u128(0x00000000_0000_0000_c000_000000000046);
    const IID_IDATAOBJECT: GUID = GUID::from_u128(0x0000010e_0000_0000_c000_000000000046);
    const IID_IDROPSOURCE: GUID = GUID::from_u128(0x00000121_0000_0000_c000_000000000046);
    const E_OUTOFMEMORY: HRESULT = 0x8007000E_u32 as _;
    const DATADIR_GET: u32 = 1;

    #[repr(C)]
    struct IUnknownVtbl {
        query_interface:
            unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
        add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
    }

    pub unsafe fn release_com(ptr: *mut c_void) {
        if !ptr.is_null() {
            let vtbl = *(ptr as *const *const IUnknownVtbl);
            ((*vtbl).release)(ptr);
        }
    }

    fn guids_equal(a: &GUID, b: &GUID) -> bool {
        a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
    }

    pub fn build_cf_hdrop_bytes(paths: &[&str]) -> Result<Vec<u8>, String> {
        if paths.is_empty() {
            return Err("At least one file path is required for CF_HDROP".to_string());
        }

        let header = DROPFILES {
            pFiles: size_of::<DROPFILES>() as u32,
            pt: POINT { x: 0, y: 0 },
            fNC: 0,
            fWide: 1,
        };

        let mut wide_chars: Vec<u16> = Vec::new();
        for p in paths {
            wide_chars.extend(p.encode_utf16());
            wide_chars.push(0);
        }
        wide_chars.push(0);

        let total_len = size_of::<DROPFILES>() + wide_chars.len() * size_of::<u16>();
        let mut out = Vec::with_capacity(total_len);
        let header_bytes = unsafe {
            std::slice::from_raw_parts(
                (&header as *const DROPFILES) as *const u8,
                size_of::<DROPFILES>(),
            )
        };
        out.extend_from_slice(header_bytes);

        let path_bytes = unsafe {
            std::slice::from_raw_parts(
                wide_chars.as_ptr() as *const u8,
                wide_chars.len() * size_of::<u16>(),
            )
        };
        out.extend_from_slice(path_bytes);
        Ok(out)
    }

    unsafe fn alloc_hglobal_from_bytes(bytes: &[u8]) -> Result<HGLOBAL, String> {
        let hmem = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
        if hmem.is_null() {
            return Err("GlobalAlloc failed for CF_HDROP payload".to_string());
        }
        let locked = GlobalLock(hmem) as *mut u8;
        if locked.is_null() {
            let _ = GlobalFree(hmem);
            return Err("GlobalLock failed for CF_HDROP payload".to_string());
        }
        copy_nonoverlapping(bytes.as_ptr(), locked, bytes.len());
        let _ = GlobalUnlock(hmem);
        Ok(hmem)
    }

    #[repr(C)]
    struct IDataObjectVtbl {
        query_interface:
            unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
        add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
        get_data:
            unsafe extern "system" fn(*mut c_void, *const FORMATETC, *mut STGMEDIUM) -> HRESULT,
        get_data_here:
            unsafe extern "system" fn(*mut c_void, *const FORMATETC, *mut STGMEDIUM) -> HRESULT,
        query_get_data: unsafe extern "system" fn(*mut c_void, *const FORMATETC) -> HRESULT,
        get_canonical_format_etc:
            unsafe extern "system" fn(*mut c_void, *const FORMATETC, *mut FORMATETC) -> HRESULT,
        set_data: unsafe extern "system" fn(
            *mut c_void,
            *const FORMATETC,
            *const STGMEDIUM,
            BOOL,
        ) -> HRESULT,
        enum_format_etc: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> HRESULT,
        d_advise: unsafe extern "system" fn(
            *mut c_void,
            *const FORMATETC,
            u32,
            *mut c_void,
            *mut u32,
        ) -> HRESULT,
        d_unadvise: unsafe extern "system" fn(*mut c_void, u32) -> HRESULT,
        enum_d_advise: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT,
    }

    #[repr(C)]
    pub struct CfHdropDataObject {
        vtbl: *const IDataObjectVtbl,
        ref_count: AtomicU32,
        hdrop_bytes: Vec<u8>,
    }

    static CF_HDROP_VTBL: IDataObjectVtbl = IDataObjectVtbl {
        query_interface: data_obj_query_interface,
        add_ref: data_obj_add_ref,
        release: data_obj_release,
        get_data: data_obj_get_data,
        get_data_here: data_obj_get_data_here,
        query_get_data: data_obj_query_get_data,
        get_canonical_format_etc: data_obj_get_canonical_format_etc,
        set_data: data_obj_set_data,
        enum_format_etc: data_obj_enum_format_etc,
        d_advise: data_obj_d_advise,
        d_unadvise: data_obj_d_unadvise,
        enum_d_advise: data_obj_enum_d_advise,
    };

    impl CfHdropDataObject {
        pub fn new_raw(path: &str) -> Result<*mut c_void, String> {
            let hdrop_bytes = build_cf_hdrop_bytes(&[path])?;
            let boxed = Box::new(Self {
                vtbl: &CF_HDROP_VTBL,
                ref_count: AtomicU32::new(1),
                hdrop_bytes,
            });
            Ok(Box::into_raw(boxed) as *mut c_void)
        }
    }

    fn matches_cf_hdrop(fmt: &FORMATETC) -> bool {
        fmt.cfFormat == CF_HDROP
            && fmt.dwAspect == DVASPECT_CONTENT
            && (fmt.lindex == -1 || fmt.lindex == 0)
            && (fmt.tymed & (TYMED_HGLOBAL as u32)) != 0
    }

    unsafe extern "system" fn data_obj_query_interface(
        this: *mut c_void,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> HRESULT {
        if ppv.is_null() || riid.is_null() {
            return E_POINTER;
        }
        let iid = &*riid;
        if guids_equal(iid, &IID_IUNKNOWN) || guids_equal(iid, &IID_IDATAOBJECT) {
            *ppv = this;
            data_obj_add_ref(this);
            S_OK
        } else {
            *ppv = null_mut();
            E_NOINTERFACE
        }
    }

    unsafe extern "system" fn data_obj_add_ref(this: *mut c_void) -> u32 {
        let obj = &*(this as *const CfHdropDataObject);
        obj.ref_count.fetch_add(1, Ordering::Relaxed) + 1
    }

    unsafe extern "system" fn data_obj_release(this: *mut c_void) -> u32 {
        let obj = &*(this as *const CfHdropDataObject);
        let prev = obj.ref_count.fetch_sub(1, Ordering::Release);
        if prev == 1 {
            std::sync::atomic::fence(Ordering::Acquire);
            drop(Box::from_raw(this as *mut CfHdropDataObject));
            0
        } else {
            prev - 1
        }
    }

    unsafe extern "system" fn data_obj_query_get_data(
        _this: *mut c_void,
        pformatetc: *const FORMATETC,
    ) -> HRESULT {
        if pformatetc.is_null() {
            return E_POINTER;
        }
        if matches_cf_hdrop(&*pformatetc) {
            S_OK
        } else {
            DV_E_FORMATETC
        }
    }

    unsafe extern "system" fn data_obj_get_data(
        this: *mut c_void,
        pformatetc_in: *const FORMATETC,
        pmedium: *mut STGMEDIUM,
    ) -> HRESULT {
        if pformatetc_in.is_null() || pmedium.is_null() {
            return E_POINTER;
        }
        if !matches_cf_hdrop(&*pformatetc_in) {
            return DV_E_FORMATETC;
        }
        let obj = &*(this as *const CfHdropDataObject);
        match alloc_hglobal_from_bytes(&obj.hdrop_bytes) {
            Ok(hglobal) => {
                *pmedium = STGMEDIUM {
                    tymed: TYMED_HGLOBAL as u32,
                    u: STGMEDIUM_0 { hGlobal: hglobal },
                    pUnkForRelease: null_mut(),
                };
                S_OK
            }
            Err(_) => E_OUTOFMEMORY,
        }
    }

    unsafe extern "system" fn data_obj_get_data_here(
        _this: *mut c_void,
        _pformatetc: *const FORMATETC,
        _pmedium: *mut STGMEDIUM,
    ) -> HRESULT {
        E_NOTIMPL
    }

    unsafe extern "system" fn data_obj_get_canonical_format_etc(
        _this: *mut c_void,
        pformatect_in: *const FORMATETC,
        pformatetc_out: *mut FORMATETC,
    ) -> HRESULT {
        if pformatect_in.is_null() || pformatetc_out.is_null() {
            return E_POINTER;
        }
        *pformatetc_out = *pformatect_in;
        (*pformatetc_out).ptd = null_mut();
        S_OK
    }

    unsafe extern "system" fn data_obj_set_data(
        _this: *mut c_void,
        _pformatetc: *const FORMATETC,
        _pmedium: *const STGMEDIUM,
        _f_release: BOOL,
    ) -> HRESULT {
        E_NOTIMPL
    }

    unsafe extern "system" fn data_obj_enum_format_etc(
        _this: *mut c_void,
        dw_direction: u32,
        ppenum_format_etc: *mut *mut c_void,
    ) -> HRESULT {
        if ppenum_format_etc.is_null() {
            return E_POINTER;
        }
        *ppenum_format_etc = null_mut();
        if dw_direction != DATADIR_GET {
            return E_NOTIMPL;
        }
        let fmt = FORMATETC {
            cfFormat: CF_HDROP,
            ptd: null_mut(),
            dwAspect: DVASPECT_CONTENT,
            lindex: -1,
            tymed: TYMED_HGLOBAL as u32,
        };
        SHCreateStdEnumFmtEtc(1, &fmt, ppenum_format_etc)
    }

    unsafe extern "system" fn data_obj_d_advise(
        _this: *mut c_void,
        _pformatetc: *const FORMATETC,
        _advf: u32,
        _p_adv_sink: *mut c_void,
        _pdw_connection: *mut u32,
    ) -> HRESULT {
        E_NOTIMPL
    }

    unsafe extern "system" fn data_obj_d_unadvise(
        _this: *mut c_void,
        _dw_connection: u32,
    ) -> HRESULT {
        E_NOTIMPL
    }

    unsafe extern "system" fn data_obj_enum_d_advise(
        _this: *mut c_void,
        ppenum_advise: *mut *mut c_void,
    ) -> HRESULT {
        if !ppenum_advise.is_null() {
            *ppenum_advise = null_mut();
        }
        E_NOTIMPL
    }

    #[repr(C)]
    struct IDropSourceVtbl {
        query_interface:
            unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
        add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
        query_continue_drag: unsafe extern "system" fn(*mut c_void, BOOL, u32) -> HRESULT,
        give_feedback: unsafe extern "system" fn(*mut c_void, DROPEFFECT) -> HRESULT,
    }

    #[repr(C)]
    pub struct SimpleDropSource {
        vtbl: *const IDropSourceVtbl,
        ref_count: AtomicU32,
    }

    static SIMPLE_DROP_SOURCE_VTBL: IDropSourceVtbl = IDropSourceVtbl {
        query_interface: drop_src_query_interface,
        add_ref: drop_src_add_ref,
        release: drop_src_release,
        query_continue_drag: drop_src_query_continue_drag,
        give_feedback: drop_src_give_feedback,
    };

    impl SimpleDropSource {
        pub fn new_raw() -> *mut c_void {
            let boxed = Box::new(Self {
                vtbl: &SIMPLE_DROP_SOURCE_VTBL,
                ref_count: AtomicU32::new(1),
            });
            Box::into_raw(boxed) as *mut c_void
        }
    }

    unsafe extern "system" fn drop_src_query_interface(
        this: *mut c_void,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> HRESULT {
        if ppv.is_null() || riid.is_null() {
            return E_POINTER;
        }
        let iid = &*riid;
        if guids_equal(iid, &IID_IUNKNOWN) || guids_equal(iid, &IID_IDROPSOURCE) {
            *ppv = this;
            drop_src_add_ref(this);
            S_OK
        } else {
            *ppv = null_mut();
            E_NOINTERFACE
        }
    }

    unsafe extern "system" fn drop_src_add_ref(this: *mut c_void) -> u32 {
        let obj = &*(this as *const SimpleDropSource);
        obj.ref_count.fetch_add(1, Ordering::Relaxed) + 1
    }

    unsafe extern "system" fn drop_src_release(this: *mut c_void) -> u32 {
        let obj = &*(this as *const SimpleDropSource);
        let prev = obj.ref_count.fetch_sub(1, Ordering::Release);
        if prev == 1 {
            std::sync::atomic::fence(Ordering::Acquire);
            drop(Box::from_raw(this as *mut SimpleDropSource));
            0
        } else {
            prev - 1
        }
    }

    pub unsafe extern "system" fn drop_src_query_continue_drag(
        _this: *mut c_void,
        f_escape_pressed: BOOL,
        grf_key_state: u32,
    ) -> HRESULT {
        if f_escape_pressed != 0 {
            return DRAGDROP_S_CANCEL;
        }
        if (grf_key_state & MK_LBUTTON) == 0 {
            return DRAGDROP_S_DROP;
        }
        S_OK
    }

    pub unsafe extern "system" fn drop_src_give_feedback(
        _this: *mut c_void,
        _dw_effect: DROPEFFECT,
    ) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }

    pub unsafe fn create_shell_data_object(path: &str) -> Result<*mut c_void, String> {
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let full_pidl = ILCreateFromPathW(wide.as_ptr());
        if full_pidl.is_null() {
            return Err("ILCreateFromPathW returned null".to_string());
        }

        let parent_pidl = ILClone(full_pidl);
        if parent_pidl.is_null() {
            ILFree(full_pidl);
            return Err("ILClone returned null".to_string());
        }

        let mut data_obj: *mut c_void = null_mut();
        let removed = ILRemoveLastID(parent_pidl);
        if removed != 0 {
            let child_pidl = ILFindLastID(full_pidl) as *const ITEMIDLIST;
            if !child_pidl.is_null() {
                let apidl = [child_pidl];
                let hr = SHCreateDataObject(
                    parent_pidl,
                    1,
                    apidl.as_ptr(),
                    null_mut(),
                    &IID_IDATAOBJECT,
                    &mut data_obj,
                );
                if hr < 0 {
                    data_obj = null_mut();
                }
            }
        }

        ILFree(parent_pidl);
        ILFree(full_pidl);

        if data_obj.is_null() {
            Err("SHCreateDataObject failed".to_string())
        } else {
            Ok(data_obj)
        }
    }

    pub fn execute_drag_on_ui_thread(hwnd: HWND, file_path: &str) -> Result<DROPEFFECT, String> {
        let normalized = validate_drag_file_path(file_path)?;

        unsafe {
            let hr_ole = OleInitialize(null());
            let mut effect: DROPEFFECT = 0;

            let hr = match create_shell_data_object(&normalized) {
                Ok(shell_data_obj) => {
                    let drop_src = SimpleDropSource::new_raw();
                    let drag_hr = SHDoDragDrop(
                        hwnd,
                        shell_data_obj,
                        drop_src,
                        DROPEFFECT_COPY | DROPEFFECT_LINK,
                        &mut effect,
                    );
                    release_com(drop_src);
                    release_com(shell_data_obj);
                    drag_hr
                }
                Err(_) => {
                    let data_obj = CfHdropDataObject::new_raw(&normalized)?;
                    let drop_src = SimpleDropSource::new_raw();
                    let drag_hr = DoDragDrop(data_obj, drop_src, DROPEFFECT_COPY, &mut effect);
                    release_com(drop_src);
                    release_com(data_obj);
                    drag_hr
                }
            };

            if hr_ole == S_OK || hr_ole == S_FALSE {
                OleUninitialize();
            }

            if hr < 0 {
                Err(format!("OLE drag operation failed (HRESULT 0x{:08X})", hr as u32))
            } else {
                Ok(effect)
            }
        }
    }
}

pub fn normalize_win32_path(raw: &str) -> PathBuf {
    let trimmed = raw.trim();
    if let Some(unc) = trimmed.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{}", unc))
    } else if let Some(clean) = trimmed.strip_prefix(r"\\?\") {
        PathBuf::from(clean)
    } else {
        PathBuf::from(trimmed)
    }
}

pub fn validate_drag_file_path(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Cannot drag download: file path is empty".to_string());
    }

    let candidate = normalize_win32_path(trimmed);
    let meta = std::fs::metadata(&candidate)
        .map_err(|_| format!("File not found on disk: {}", candidate.display()))?;
    if !meta.is_file() {
        return Err(format!("Target path is not a file: {}", candidate.display()));
    }

    let resolved = candidate.canonicalize().unwrap_or_else(|_| Path::new(&candidate).to_path_buf());
    Ok(normalize_win32_path(&resolved.to_string_lossy())
        .to_string_lossy()
        .to_string())
}

pub async fn start_file_drag(
    app_handle: &tauri::AppHandle,
    file_path: String,
) -> Result<(), String> {
    let validated = validate_drag_file_path(&file_path)?;

    #[cfg(target_os = "windows")]
    {
        use tauri::Manager;

        let main_window = app_handle
            .get_webview_window("main")
            .ok_or_else(|| "Main window handle unavailable for file drag".to_string())?;

        let (tx, rx) = tokio::sync::oneshot::channel();
        let win_for_cb = main_window.clone();
        main_window
            .run_on_main_thread(move || {
                let hwnd = win_for_cb
                    .hwnd()
                    .map(|h| h.0 as windows_sys::Win32::Foundation::HWND)
                    .unwrap_or(std::ptr::null_mut());
                let res = win_ole::execute_drag_on_ui_thread(hwnd, &validated).map(|_| ());
                let _ = tx.send(res);
            })
            .map_err(|e| format!("Failed scheduling OLE drag on UI thread: {}", e))?;

        rx.await
            .map_err(|e| format!("OLE drag completion channel closed: {}", e))?
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app_handle, validated);
        Err("Outbound OS file drag is only implemented for Windows".to_string())
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::win_ole::{
        build_cf_hdrop_bytes, drop_src_give_feedback, drop_src_query_continue_drag, release_com,
        CfHdropDataObject, SimpleDropSource,
    };
    use super::{normalize_win32_path, validate_drag_file_path};
    use std::mem::size_of;
    use uuid::Uuid;
    use windows_sys::Win32::Foundation::{
        DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, S_OK,
    };
    use windows_sys::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, STGMEDIUM, TYMED_HGLOBAL};
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    use windows_sys::Win32::System::Ole::{
        OleInitialize, OleUninitialize, ReleaseStgMedium, CF_HDROP, DROPEFFECT_COPY,
    };
    use windows_sys::Win32::System::SystemServices::MK_LBUTTON;
    use windows_sys::Win32::UI::Shell::DROPFILES;

    #[test]
    fn test_normalize_and_validate_drag_file_path() {
        let p = normalize_win32_path(r"\\?\C:\Users\Twix\Downloads\test.zip");
        assert_eq!(p.to_string_lossy(), r"C:\Users\Twix\Downloads\test.zip");

        let unc = normalize_win32_path(r"\\?\UNC\server\share\video.mp4");
        assert_eq!(unc.to_string_lossy(), r"\\server\share\video.mp4");

        let temp_file = std::env::temp_dir().join(format!("octoheft_drag_{}.bin", Uuid::new_v4()));
        std::fs::write(&temp_file, b"hello").unwrap();

        let validated = validate_drag_file_path(&temp_file.to_string_lossy())
            .expect("existing file should validate");
        assert!(!validated.starts_with(r"\\?\"));
        assert!(validated.ends_with(".bin"));

        let _ = std::fs::remove_file(&temp_file);
        assert!(validate_drag_file_path(&temp_file.to_string_lossy()).is_err());
    }

    #[test]
    fn test_cf_hdrop_payload_and_com_data_object() {
        let temp_file = std::env::temp_dir().join(format!("octoheft_hdrop_{}.txt", Uuid::new_v4()));
        std::fs::write(&temp_file, b"payload").unwrap();
        let clean_path = validate_drag_file_path(&temp_file.to_string_lossy()).unwrap();

        let bytes = build_cf_hdrop_bytes(&[&clean_path]).unwrap();
        assert_eq!(size_of::<DROPFILES>(), 20);
        let dropfiles =
            unsafe { std::ptr::read_unaligned(bytes.as_ptr() as *const DROPFILES) };
        let p_files = dropfiles.pFiles;
        let f_wide = dropfiles.fWide;
        assert_eq!(p_files, 20);
        assert_eq!(f_wide, 1);

        let wide_slice = unsafe {
            std::slice::from_raw_parts(
                bytes.as_ptr().add(20) as *const u16,
                (bytes.len() - 20) / 2,
            )
        };
        assert_eq!(wide_slice[wide_slice.len() - 2], 0);
        assert_eq!(wide_slice[wide_slice.len() - 1], 0);
        let decoded = String::from_utf16_lossy(&wide_slice[..wide_slice.len() - 2]);
        assert_eq!(decoded, clean_path);

        unsafe {
            let hr_ole = OleInitialize(std::ptr::null());
            let data_obj = CfHdropDataObject::new_raw(&clean_path).unwrap();
            let vtbl = &*(*(data_obj as *const *const *const usize));
            let _ = vtbl;

            // Exercise QueryGetData + GetData directly via IDataObject vtable slots
            #[repr(C)]
            struct RawDataObj {
                vtbl: *const DataObjVtblCheck,
            }
            #[repr(C)]
            struct DataObjVtblCheck {
                qi: usize,
                add_ref: usize,
                release: usize,
                get_data: unsafe extern "system" fn(
                    *mut std::ffi::c_void,
                    *const FORMATETC,
                    *mut STGMEDIUM,
                ) -> windows_sys::core::HRESULT,
                get_data_here: usize,
                query_get_data: unsafe extern "system" fn(
                    *mut std::ffi::c_void,
                    *const FORMATETC,
                ) -> windows_sys::core::HRESULT,
            }

            let raw = &*(data_obj as *const RawDataObj);
            let fmt = FORMATETC {
                cfFormat: CF_HDROP,
                ptd: std::ptr::null_mut(),
                dwAspect: DVASPECT_CONTENT,
                lindex: -1,
                tymed: TYMED_HGLOBAL as u32,
            };
            assert_eq!(((*raw.vtbl).query_get_data)(data_obj, &fmt), S_OK);

            let mut med: STGMEDIUM = std::mem::zeroed();
            assert_eq!(((*raw.vtbl).get_data)(data_obj, &fmt, &mut med), S_OK);
            assert_eq!(med.tymed, TYMED_HGLOBAL as u32);

            let locked = GlobalLock(med.u.hGlobal) as *const DROPFILES;
            assert!(!locked.is_null());
            let locked_copy = std::ptr::read_unaligned(locked);
            let locked_wide = locked_copy.fWide;
            assert_eq!(locked_wide, 1);
            let _ = GlobalUnlock(med.u.hGlobal);
            ReleaseStgMedium(&mut med);
            release_com(data_obj);

            // Also verify Shell SHCreateDataObject creation on real file
            let shell_obj = super::win_ole::create_shell_data_object(&clean_path)
                .expect("SHCreateDataObject should succeed on valid real file");
            release_com(shell_obj);

            // Exercise IDropSource state transitions
            let drop_src = SimpleDropSource::new_raw();
            assert_eq!(
                drop_src_query_continue_drag(drop_src, 1, MK_LBUTTON),
                DRAGDROP_S_CANCEL
            );
            assert_eq!(drop_src_query_continue_drag(drop_src, 0, 0), DRAGDROP_S_DROP);
            assert_eq!(
                drop_src_query_continue_drag(drop_src, 0, MK_LBUTTON),
                S_OK
            );
            assert_eq!(
                drop_src_give_feedback(drop_src, DROPEFFECT_COPY),
                DRAGDROP_S_USEDEFAULTCURSORS
            );
            release_com(drop_src);

            if hr_ole >= 0 {
                OleUninitialize();
            }
        }

        let _ = std::fs::remove_file(&temp_file);
    }
}
