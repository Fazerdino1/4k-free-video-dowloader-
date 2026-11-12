#ifndef UNICODE
#define UNICODE
#endif
#ifndef _UNICODE
#define _UNICODE
#endif
#define COBJMACROS
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <commctrl.h>
#include <shlobj.h>
#include <objbase.h>
#include <shobjidl.h>
#include <shellapi.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <wchar.h>

#define IDI_APPICON     101
#define IDC_PATH_EDIT   201
#define IDC_BROWSE_BTN  202
#define IDC_CHK_START   203
#define IDC_CHK_DESKTOP 204
#define IDC_CHK_LAUNCH  205
#define IDC_PROGRESS    206
#define IDC_STATUS_LBL  207
#define IDC_INSTALL_BTN 208
#define IDC_CANCEL_BTN  209

#define APP_NAME        L"4k free video dowloader"
#define APP_VERSION     L"0.3.0"
#define APP_PUBLISHER   L"Fazerdino1"
#define MAIN_EXE_NAME   L"4k free video dowloader.exe"
#define UNINSTALL_KEY   L"Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\4kFreeVideoDownloader"

static HWND g_hWnd = NULL;
static HWND g_hEditPath = NULL;
static HWND g_hChkStart = NULL;
static HWND g_hChkDesktop = NULL;
static HWND g_hChkLaunch = NULL;
static HWND g_hProgress = NULL;
static HWND g_hStatus = NULL;
static HWND g_hBtnInstall = NULL;
static HFONT g_hFontTitle = NULL;
static HFONT g_hFontSub = NULL;
static HFONT g_hFontUI = NULL;

static uint64_t fnv1a64(const uint8_t *data, size_t len) {
    uint64_t h = 14695981039346656037ULL;
    for (size_t i = 0; i < len; i++) {
        h ^= (uint64_t)data[i];
        h *= 1099511628211ULL;
    }
    return h;
}

static int lz4_decompress_block(const uint8_t *src, size_t src_len, uint8_t *dst, size_t dst_cap, size_t *out_len) {
    size_t ip = 0;
    size_t op = 0;
    while (ip < src_len) {
        uint8_t token = src[ip++];
        size_t lit_len = token >> 4;
        if (lit_len == 15) {
            uint8_t s;
            do {
                if (ip >= src_len) return -1;
                s = src[ip++];
                lit_len += s;
            } while (s == 255);
        }
        if (ip + lit_len > src_len || op + lit_len > dst_cap) return -2;
        memcpy(dst + op, src + ip, lit_len);
        ip += lit_len;
        op += lit_len;

        if (ip >= src_len) break;

        if (ip + 2 > src_len) return -3;
        uint16_t offset = (uint16_t)src[ip] | ((uint16_t)src[ip + 1] << 8);
        ip += 2;
        if (offset == 0 || offset > op) return -4;

        size_t match_len = (token & 0x0F) + 4;
        if ((token & 0x0F) == 15) {
            uint8_t s;
            do {
                if (ip >= src_len) return -5;
                s = src[ip++];
                match_len += s;
            } while (s == 255);
        }
        if (op + match_len > dst_cap) return -6;
        size_t ref = op - offset;
        for (size_t i = 0; i < match_len; i++) {
            dst[op + i] = dst[ref + i];
        }
        op += match_len;
    }
    *out_len = op;
    return 0;
}

static void get_default_install_dir(wchar_t *out_dir, size_t cap) {
    wchar_t pf[MAX_PATH];
    if (GetEnvironmentVariableW(L"ProgramW6432", pf, MAX_PATH) > 0 ||
        GetEnvironmentVariableW(L"ProgramFiles", pf, MAX_PATH) > 0) {
        wsprintfW(out_dir, L"%s\\%s", pf, APP_NAME);
        return;
    }
    if (SUCCEEDED(SHGetFolderPathW(NULL, CSIDL_PROGRAM_FILES, NULL, 0, pf))) {
        wsprintfW(out_dir, L"%s\\%s", pf, APP_NAME);
        return;
    }
    lstrcpynW(out_dir, L"C:\\Program Files\\4k free video dowloader", (int)cap);
}

static void get_fallback_user_dir(wchar_t *out_dir) {
    wchar_t local_app[MAX_PATH];
    if (SUCCEEDED(SHGetFolderPathW(NULL, CSIDL_LOCAL_APPDATA, NULL, 0, local_app))) {
        wsprintfW(out_dir, L"%s\\Programs\\%s", local_app, APP_NAME);
    } else {
        lstrcpyW(out_dir, L"C:\\4k free video dowloader");
    }
}

static BOOL ensure_directory_recursive(const wchar_t *dir_path) {
    wchar_t tmp[MAX_PATH];
    lstrcpynW(tmp, dir_path, MAX_PATH);
    size_t len = wcslen(tmp);
    if (len == 0) return FALSE;
    if (tmp[len - 1] == L'\\') tmp[len - 1] = L'\0';

    for (wchar_t *p = tmp + 3; *p; p++) {
        if (*p == L'\\') {
            *p = L'\0';
            CreateDirectoryW(tmp, NULL);
            *p = L'\\';
        }
    }
    if (CreateDirectoryW(tmp, NULL) || GetLastError() == ERROR_ALREADY_EXISTS) {
        wchar_t probe[MAX_PATH];
        wsprintfW(probe, L"%s\\.vg_write_test", tmp);
        HANDLE h = CreateFileW(probe, GENERIC_WRITE, 0, NULL, CREATE_ALWAYS, FILE_ATTRIBUTE_TEMPORARY, NULL);
        if (h != INVALID_HANDLE_VALUE) {
            CloseHandle(h);
            DeleteFileW(probe);
            return TRUE;
        }
    }
    return FALSE;
}

static BOOL create_shortcut(const wchar_t *target_exe, const wchar_t *work_dir, const wchar_t *lnk_path) {
    IShellLinkW *psl = NULL;
    HRESULT hr = CoCreateInstance(&CLSID_ShellLink, NULL, CLSCTX_INPROC_SERVER, &IID_IShellLinkW, (LPVOID *)&psl);
    if (FAILED(hr) || !psl) return FALSE;

    IShellLinkW_SetPath(psl, target_exe);
    IShellLinkW_SetWorkingDirectory(psl, work_dir);
    IShellLinkW_SetIconLocation(psl, target_exe, 0);
    IShellLinkW_SetDescription(psl, L"4k free video dowloader — Téléchargeur vidéo 4K/8K & audio");

    IPersistFile *ppf = NULL;
    hr = IShellLinkW_QueryInterface(psl, &IID_IPersistFile, (LPVOID *)&ppf);
    if (SUCCEEDED(hr) && ppf) {
        hr = IPersistFile_Save(ppf, lnk_path, TRUE);
        IPersistFile_Release(ppf);
    }
    IShellLinkW_Release(psl);
    return SUCCEEDED(hr);
}

static void register_uninstaller(const wchar_t *install_dir, DWORD estimated_kb) {
    HKEY hKey = NULL;
    if (RegCreateKeyExW(HKEY_LOCAL_MACHINE, UNINSTALL_KEY, 0, NULL, 0, KEY_WRITE, NULL, &hKey, NULL) != ERROR_SUCCESS) {
        RegCreateKeyExW(HKEY_CURRENT_USER, UNINSTALL_KEY, 0, NULL, 0, KEY_WRITE, NULL, &hKey, NULL);
    }
    if (!hKey) return;

    wchar_t display_icon[MAX_PATH];
    wchar_t uninstall_str[MAX_PATH];
    wsprintfW(display_icon, L"%s\\%s,0", install_dir, MAIN_EXE_NAME);
    wsprintfW(uninstall_str, L"\"%s\\uninstall.exe\"", install_dir);

    RegSetValueExW(hKey, L"DisplayName", 0, REG_SZ, (const BYTE *)APP_NAME, (DWORD)((wcslen(APP_NAME) + 1) * sizeof(wchar_t)));
    RegSetValueExW(hKey, L"DisplayVersion", 0, REG_SZ, (const BYTE *)APP_VERSION, (DWORD)((wcslen(APP_VERSION) + 1) * sizeof(wchar_t)));
    RegSetValueExW(hKey, L"Publisher", 0, REG_SZ, (const BYTE *)APP_PUBLISHER, (DWORD)((wcslen(APP_PUBLISHER) + 1) * sizeof(wchar_t)));
    RegSetValueExW(hKey, L"InstallLocation", 0, REG_SZ, (const BYTE *)install_dir, (DWORD)((wcslen(install_dir) + 1) * sizeof(wchar_t)));
    RegSetValueExW(hKey, L"DisplayIcon", 0, REG_SZ, (const BYTE *)display_icon, (DWORD)((wcslen(display_icon) + 1) * sizeof(wchar_t)));
    RegSetValueExW(hKey, L"UninstallString", 0, REG_SZ, (const BYTE *)uninstall_str, (DWORD)((wcslen(uninstall_str) + 1) * sizeof(wchar_t)));

    DWORD one = 1;
    RegSetValueExW(hKey, L"NoModify", 0, REG_DWORD, (const BYTE *)&one, sizeof(DWORD));
    RegSetValueExW(hKey, L"NoRepair", 0, REG_DWORD, (const BYTE *)&one, sizeof(DWORD));
    RegSetValueExW(hKey, L"EstimatedSize", 0, REG_DWORD, (const BYTE *)&estimated_kb, sizeof(DWORD));
    RegCloseKey(hKey);
}

static BOOL read_exact(HANDLE hFile, void *buf, DWORD len) {
    BYTE *p = (BYTE *)buf;
    DWORD rem = len;
    while (rem > 0) {
        DWORD got = 0;
        if (!ReadFile(hFile, p, rem, &got, NULL) || got == 0) return FALSE;
        p += got;
        rem -= got;
    }
    return TRUE;
}

static int perform_installation(wchar_t *install_dir, BOOL make_start_menu, BOOL make_desktop, BOOL silent) {
    HWND hRunning = FindWindowW(NULL, APP_NAME);
    if (hRunning) {
        PostMessageW(hRunning, WM_CLOSE, 0, 0);
        Sleep(500);
    }

    if (!ensure_directory_recursive(install_dir)) {
        wchar_t fallback[MAX_PATH];
        get_fallback_user_dir(fallback);
        if (!ensure_directory_recursive(fallback)) {
            if (!silent) {
                MessageBoxW(g_hWnd, L"Impossible de créer le dossier d'installation.", L"Erreur", MB_ICONERROR);
            }
            return 1;
        }
        lstrcpyW(install_dir, fallback);
        if (g_hEditPath) SetWindowTextW(g_hEditPath, install_dir);
    }

    wchar_t self_path[MAX_PATH];
    GetModuleFileNameW(NULL, self_path, MAX_PATH);
    HANDLE hSelf = CreateFileW(self_path, GENERIC_READ, FILE_SHARE_READ, NULL, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, NULL);
    if (hSelf == INVALID_HANDLE_VALUE) {
        if (!silent) MessageBoxW(g_hWnd, L"Impossible d'ouvrir l'archive de l'installateur.", L"Erreur", MB_ICONERROR);
        return 2;
    }

    LARGE_INTEGER file_sz;
    if (!GetFileSizeEx(hSelf, &file_sz) || file_sz.QuadPart < 24) {
        CloseHandle(hSelf);
        return 3;
    }

    LARGE_INTEGER footer_pos;
    footer_pos.QuadPart = file_sz.QuadPart - 24;
    SetFilePointerEx(hSelf, footer_pos, NULL, FILE_BEGIN);

    uint64_t payload_offset = 0;
    uint64_t file_count = 0;
    char magic[8];
    if (!read_exact(hSelf, &payload_offset, 8) ||
        !read_exact(hSelf, &file_count, 8) ||
        !read_exact(hSelf, magic, 8) ||
        memcmp(magic, "VGSETUP1", 8) != 0) {
        CloseHandle(hSelf);
        if (!silent) MessageBoxW(g_hWnd, L"Archive d'installation invalide ou corrompue.", L"Erreur", MB_ICONERROR);
        return 4;
    }

    LARGE_INTEGER start_pos;
    start_pos.QuadPart = (LONGLONG)payload_offset;
    SetFilePointerEx(hSelf, start_pos, NULL, FILE_BEGIN);

    if (g_hProgress) {
        SendMessageW(g_hProgress, PBM_SETRANGE, 0, MAKELPARAM(0, (int)(file_count + 2)));
        SendMessageW(g_hProgress, PBM_SETPOS, 0, 0);
    }

    uint64_t total_raw_bytes = 0;

    for (uint64_t i = 0; i < file_count; i++) {
        uint32_t name_len = 0;
        if (!read_exact(hSelf, &name_len, 4) || name_len == 0 || name_len > 240) {
            CloseHandle(hSelf);
            return 5;
        }
        char name_utf8[256];
        if (!read_exact(hSelf, name_utf8, name_len)) {
            CloseHandle(hSelf);
            return 6;
        }
        name_utf8[name_len] = '\0';

        uint64_t raw_size = 0, comp_size = 0, expected_hash = 0;
        if (!read_exact(hSelf, &raw_size, 8) ||
            !read_exact(hSelf, &comp_size, 8) ||
            !read_exact(hSelf, &expected_hash, 8)) {
            CloseHandle(hSelf);
            return 7;
        }

        wchar_t wname[MAX_PATH];
        MultiByteToWideChar(CP_UTF8, 0, name_utf8, -1, wname, MAX_PATH);

        if (g_hStatus) {
            wchar_t status_msg[MAX_PATH + 64];
            wsprintfW(status_msg, L"Installation de %s...", wname);
            SetWindowTextW(g_hStatus, status_msg);
            UpdateWindow(g_hWnd);
        }

        uint8_t *comp_buf = (uint8_t *)HeapAlloc(GetProcessHeap(), 0, (SIZE_T)comp_size + 1);
        uint8_t *raw_buf = (uint8_t *)HeapAlloc(GetProcessHeap(), 0, (SIZE_T)raw_size + 1);
        if (!comp_buf || !raw_buf) {
            if (comp_buf) HeapFree(GetProcessHeap(), 0, comp_buf);
            if (raw_buf) HeapFree(GetProcessHeap(), 0, raw_buf);
            CloseHandle(hSelf);
            return 8;
        }

        if (!read_exact(hSelf, comp_buf, (DWORD)comp_size)) {
            HeapFree(GetProcessHeap(), 0, comp_buf);
            HeapFree(GetProcessHeap(), 0, raw_buf);
            CloseHandle(hSelf);
            return 9;
        }

        size_t dec_len = 0;
        int rc = lz4_decompress_block(comp_buf, (size_t)comp_size, raw_buf, (size_t)raw_size, &dec_len);
        HeapFree(GetProcessHeap(), 0, comp_buf);

        if (rc != 0 || dec_len != (size_t)raw_size || fnv1a64(raw_buf, dec_len) != expected_hash) {
            HeapFree(GetProcessHeap(), 0, raw_buf);
            CloseHandle(hSelf);
            if (!silent) MessageBoxW(g_hWnd, L"Échec du contrôle d'intégrité lors de l'extraction.", L"Erreur", MB_ICONERROR);
            return 10;
        }

        wchar_t out_path[MAX_PATH];
        wsprintfW(out_path, L"%s\\%s", install_dir, wname);
        SetFileAttributesW(out_path, FILE_ATTRIBUTE_NORMAL);
        HANDLE hOut = CreateFileW(out_path, GENERIC_WRITE, 0, NULL, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
        if (hOut == INVALID_HANDLE_VALUE) {
            HeapFree(GetProcessHeap(), 0, raw_buf);
            CloseHandle(hSelf);
            if (!silent) {
                wchar_t err_msg[MAX_PATH + 128];
                wsprintfW(err_msg, L"Impossible d'écrire le fichier :\n%s\n\nFermez l'application si elle est ouverte.", out_path);
                MessageBoxW(g_hWnd, err_msg, L"Erreur d'écriture", MB_ICONERROR);
            }
            return 11;
        }

        DWORD written = 0;
        WriteFile(hOut, raw_buf, (DWORD)raw_size, &written, NULL);
        CloseHandle(hOut);
        HeapFree(GetProcessHeap(), 0, raw_buf);

        total_raw_bytes += raw_size;
        if (g_hProgress) {
            SendMessageW(g_hProgress, PBM_SETPOS, (WPARAM)(i + 1), 0);
        }
    }

    CloseHandle(hSelf);

    wchar_t main_exe_path[MAX_PATH];
    wsprintfW(main_exe_path, L"%s\\%s", install_dir, MAIN_EXE_NAME);

    if (make_start_menu) {
        wchar_t sm_dir[MAX_PATH];
        wchar_t lnk[MAX_PATH];
        if (SUCCEEDED(SHGetFolderPathW(NULL, CSIDL_COMMON_PROGRAMS, NULL, 0, sm_dir))) {
            wsprintfW(lnk, L"%s\\4k free video dowloader.lnk", sm_dir);
            if (!create_shortcut(main_exe_path, install_dir, lnk)) {
                if (SUCCEEDED(SHGetFolderPathW(NULL, CSIDL_PROGRAMS, NULL, 0, sm_dir))) {
                    wsprintfW(lnk, L"%s\\4k free video dowloader.lnk", sm_dir);
                    create_shortcut(main_exe_path, install_dir, lnk);
                }
            }
        } else if (SUCCEEDED(SHGetFolderPathW(NULL, CSIDL_PROGRAMS, NULL, 0, sm_dir))) {
            wsprintfW(lnk, L"%s\\4k free video dowloader.lnk", sm_dir);
            create_shortcut(main_exe_path, install_dir, lnk);
        }
    }

    if (make_desktop) {
        wchar_t desk_dir[MAX_PATH];
        if (SUCCEEDED(SHGetFolderPathW(NULL, CSIDL_DESKTOPDIRECTORY, NULL, 0, desk_dir))) {
            wchar_t lnk[MAX_PATH];
            wsprintfW(lnk, L"%s\\4k free video dowloader.lnk", desk_dir);
            create_shortcut(main_exe_path, install_dir, lnk);
        }
    }

    register_uninstaller(install_dir, (DWORD)(total_raw_bytes / 1024));

    if (g_hProgress) {
        SendMessageW(g_hProgress, PBM_SETPOS, (WPARAM)(file_count + 2), 0);
    }
    if (g_hStatus) {
        SetWindowTextW(g_hStatus, L"Installation terminée avec succès !");
    }

    return 0;
}

static LRESULT CALLBACK WndProc(HWND hWnd, UINT msg, WPARAM wParam, LPARAM lParam) {
    switch (msg) {
        case WM_CREATE: {
            g_hFontTitle = CreateFontW(-20, 0, 0, 0, FW_BOLD, FALSE, FALSE, FALSE,
                DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY,
                DEFAULT_PITCH | FF_SWISS, L"Segoe UI");
            g_hFontSub = CreateFontW(-13, 0, 0, 0, FW_NORMAL, FALSE, FALSE, FALSE,
                DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY,
                DEFAULT_PITCH | FF_SWISS, L"Segoe UI");
            g_hFontUI = CreateFontW(-14, 0, 0, 0, FW_NORMAL, FALSE, FALSE, FALSE,
                DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY,
                DEFAULT_PITCH | FF_SWISS, L"Segoe UI");

            HICON hIcon = LoadIconW(GetModuleHandleW(NULL), MAKEINTRESOURCEW(IDI_APPICON));
            if (hIcon) {
                SendMessageW(hWnd, WM_SETICON, ICON_BIG, (LPARAM)hIcon);
                SendMessageW(hWnd, WM_SETICON, ICON_SMALL, (LPARAM)hIcon);
            }

            HWND hIconPic = CreateWindowExW(0, L"STATIC", NULL, WS_CHILD | WS_VISIBLE | SS_ICON,
                22, 18, 32, 32, hWnd, NULL, NULL, NULL);
            if (hIcon) SendMessageW(hIconPic, STM_SETICON, (WPARAM)hIcon, 0);

            HWND hTitle = CreateWindowExW(0, L"STATIC", L"4k free video dowloader v0.3.0",
                WS_CHILD | WS_VISIBLE, 68, 16, 430, 28, hWnd, NULL, NULL, NULL);
            SendMessageW(hTitle, WM_SETFONT, (WPARAM)g_hFontTitle, TRUE);

            HWND hSub = CreateWindowExW(0, L"STATIC",
                L"Installe de façon permanente 4k free video dowloader.exe, WebView2Loader.dll\n"
                L"et vidgrab-cli.exe dans vos Programmes Windows avec désinstallateur intégré.",
                WS_CHILD | WS_VISIBLE, 68, 44, 430, 38, hWnd, NULL, NULL, NULL);
            SendMessageW(hSub, WM_SETFONT, (WPARAM)g_hFontSub, TRUE);

            HWND hLblDir = CreateWindowExW(0, L"STATIC", L"Dossier d'installation dans Programmes :",
                WS_CHILD | WS_VISIBLE, 22, 96, 470, 20, hWnd, NULL, NULL, NULL);
            SendMessageW(hLblDir, WM_SETFONT, (WPARAM)g_hFontUI, TRUE);

            wchar_t default_dir[MAX_PATH];
            get_default_install_dir(default_dir, MAX_PATH);

            g_hEditPath = CreateWindowExW(WS_EX_CLIENTEDGE, L"EDIT", default_dir,
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL,
                22, 118, 365, 26, hWnd, (HMENU)IDC_PATH_EDIT, NULL, NULL);
            SendMessageW(g_hEditPath, WM_SETFONT, (WPARAM)g_hFontUI, TRUE);

            HWND hBrowse = CreateWindowExW(0, L"BUTTON", L"Parcourir...",
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON,
                395, 117, 102, 28, hWnd, (HMENU)IDC_BROWSE_BTN, NULL, NULL);
            SendMessageW(hBrowse, WM_SETFONT, (WPARAM)g_hFontUI, TRUE);

            g_hChkStart = CreateWindowExW(0, L"BUTTON", L"Créer un raccourci dans le Menu Démarrer (Programmes)",
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_AUTOCHECKBOX,
                22, 158, 470, 22, hWnd, (HMENU)IDC_CHK_START, NULL, NULL);
            SendMessageW(g_hChkStart, WM_SETFONT, (WPARAM)g_hFontUI, TRUE);
            SendMessageW(g_hChkStart, BM_SETCHECK, BST_CHECKED, 0);

            g_hChkDesktop = CreateWindowExW(0, L"BUTTON", L"Créer un raccourci sur le Bureau",
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_AUTOCHECKBOX,
                22, 184, 470, 22, hWnd, (HMENU)IDC_CHK_DESKTOP, NULL, NULL);
            SendMessageW(g_hChkDesktop, WM_SETFONT, (WPARAM)g_hFontUI, TRUE);
            SendMessageW(g_hChkDesktop, BM_SETCHECK, BST_CHECKED, 0);

            g_hChkLaunch = CreateWindowExW(0, L"BUTTON", L"Lancer 4k free video dowloader à la fin de l'installation",
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_AUTOCHECKBOX,
                22, 210, 470, 22, hWnd, (HMENU)IDC_CHK_LAUNCH, NULL, NULL);
            SendMessageW(g_hChkLaunch, WM_SETFONT, (WPARAM)g_hFontUI, TRUE);
            SendMessageW(g_hChkLaunch, BM_SETCHECK, BST_CHECKED, 0);

            g_hStatus = CreateWindowExW(0, L"STATIC", L"Prêt à installer dans vos Programmes Windows.",
                WS_CHILD | WS_VISIBLE, 22, 244, 475, 20, hWnd, (HMENU)IDC_STATUS_LBL, NULL, NULL);
            SendMessageW(g_hStatus, WM_SETFONT, (WPARAM)g_hFontSub, TRUE);

            g_hProgress = CreateWindowExW(0, PROGRESS_CLASSW, NULL,
                WS_CHILD | WS_VISIBLE | PBS_SMOOTH,
                22, 266, 475, 18, hWnd, (HMENU)IDC_PROGRESS, NULL, NULL);

            g_hBtnInstall = CreateWindowExW(0, L"BUTTON", L"Installer",
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_DEFPUSHBUTTON,
                275, 298, 110, 32, hWnd, (HMENU)IDC_INSTALL_BTN, NULL, NULL);
            SendMessageW(g_hBtnInstall, WM_SETFONT, (WPARAM)g_hFontUI, TRUE);

            HWND hCancel = CreateWindowExW(0, L"BUTTON", L"Quitter",
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON,
                395, 298, 102, 32, hWnd, (HMENU)IDC_CANCEL_BTN, NULL, NULL);
            SendMessageW(hCancel, WM_SETFONT, (WPARAM)g_hFontUI, TRUE);
            return 0;
        }

        case WM_COMMAND: {
            WORD id = LOWORD(wParam);
            if (id == IDC_BROWSE_BTN) {
                BROWSEINFOW bi;
                ZeroMemory(&bi, sizeof(bi));
                bi.hwndOwner = hWnd;
                bi.lpszTitle = L"Sélectionnez le dossier d'installation :";
                bi.ulFlags = BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE;
                LPITEMIDLIST pidl = SHBrowseForFolderW(&bi);
                if (pidl) {
                    wchar_t sel[MAX_PATH];
                    if (SHGetPathFromIDListW(pidl, sel)) {
                        wchar_t full[MAX_PATH];
                        wsprintfW(full, L"%s\\%s", sel, APP_NAME);
                        SetWindowTextW(g_hEditPath, full);
                    }
                    CoTaskMemFree(pidl);
                }
                return 0;
            }
            if (id == IDC_INSTALL_BTN) {
                EnableWindow(g_hBtnInstall, FALSE);
                wchar_t install_dir[MAX_PATH];
                GetWindowTextW(g_hEditPath, install_dir, MAX_PATH);
                BOOL make_sm = (SendMessageW(g_hChkStart, BM_GETCHECK, 0, 0) == BST_CHECKED);
                BOOL make_dt = (SendMessageW(g_hChkDesktop, BM_GETCHECK, 0, 0) == BST_CHECKED);
                BOOL launch = (SendMessageW(g_hChkLaunch, BM_GETCHECK, 0, 0) == BST_CHECKED);

                int rc = perform_installation(install_dir, make_sm, make_dt, FALSE);
                if (rc == 0) {
                    if (launch) {
                        wchar_t exe_path[MAX_PATH];
                        wsprintfW(exe_path, L"%s\\%s", install_dir, MAIN_EXE_NAME);
                        ShellExecuteW(NULL, L"open", exe_path, NULL, install_dir, SW_SHOWNORMAL);
                    } else {
                        MessageBoxW(hWnd,
                            L"4k free video dowloader v0.3.0 a été installé avec succès dans vos Programmes Windows !",
                            L"Installation terminée",
                            MB_ICONINFORMATION | MB_OK);
                    }
                    DestroyWindow(hWnd);
                } else {
                    EnableWindow(g_hBtnInstall, TRUE);
                }
                return 0;
            }
            if (id == IDC_CANCEL_BTN) {
                DestroyWindow(hWnd);
                return 0;
            }
            break;
        }

        case WM_DESTROY:
            if (g_hFontTitle) DeleteObject(g_hFontTitle);
            if (g_hFontSub) DeleteObject(g_hFontSub);
            if (g_hFontUI) DeleteObject(g_hFontUI);
            PostQuitMessage(0);
            return 0;
    }
    return DefWindowProcW(hWnd, msg, wParam, lParam);
}

int WINAPI wWinMain(HINSTANCE hInstance, HINSTANCE hPrevInstance, LPWSTR lpCmdLine, int nCmdShow) {
    (void)hPrevInstance;
    CoInitializeEx(NULL, COINIT_APARTMENTTHREADED);

    INITCOMMONCONTROLSEX icex;
    icex.dwSize = sizeof(icex);
    icex.dwICC = ICC_PROGRESS_CLASS | ICC_STANDARD_CLASSES;
    InitCommonControlsEx(&icex);

    if (lpCmdLine && (wcsstr(lpCmdLine, L"/S") || wcsstr(lpCmdLine, L"/s") || wcsstr(lpCmdLine, L"--silent"))) {
        wchar_t install_dir[MAX_PATH];
        get_default_install_dir(install_dir, MAX_PATH);
        int rc = perform_installation(install_dir, TRUE, TRUE, TRUE);
        CoUninitialize();
        return rc;
    }

    WNDCLASSEXW wc;
    ZeroMemory(&wc, sizeof(wc));
    wc.cbSize = sizeof(wc);
    wc.lpfnWndProc = WndProc;
    wc.hInstance = hInstance;
    wc.hCursor = LoadCursor(NULL, IDC_ARROW);
    wc.hbrBackground = (HBRUSH)(COLOR_BTNFACE + 1);
    wc.lpszClassName = L"VidGrabSetupWnd";
    wc.hIcon = LoadIconW(hInstance, MAKEINTRESOURCEW(IDI_APPICON));
    wc.hIconSm = wc.hIcon;
    RegisterClassExW(&wc);

    int w = 528, h = 382;
    int sx = (GetSystemMetrics(SM_CXSCREEN) - w) / 2;
    int sy = (GetSystemMetrics(SM_CYSCREEN) - h) / 2;

    g_hWnd = CreateWindowExW(
        WS_EX_DLGMODALFRAME,
        L"VidGrabSetupWnd",
        L"Installation — 4k free video dowloader v0.3.0",
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
        sx > 0 ? sx : CW_USEDEFAULT,
        sy > 0 ? sy : CW_USEDEFAULT,
        w, h,
        NULL, NULL, hInstance, NULL
    );

    ShowWindow(g_hWnd, nCmdShow);
    UpdateWindow(g_hWnd);

    MSG msg;
    while (GetMessageW(&msg, NULL, 0, 0) > 0) {
        if (!IsDialogMessageW(g_hWnd, &msg)) {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    CoUninitialize();
    return 0;
}
