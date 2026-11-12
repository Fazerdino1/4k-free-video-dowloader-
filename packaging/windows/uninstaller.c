#ifndef UNICODE
#define UNICODE
#endif
#ifndef _UNICODE
#define _UNICODE
#endif
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <shlobj.h>
#include <shellapi.h>
#include <wchar.h>

#define UNINSTALL_REG_KEY L"Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\4kFreeVideoDownloader"

static void remove_shortcut_in_folder(int csidl, const wchar_t *lnk_name) {
    wchar_t dir[MAX_PATH];
    if (SUCCEEDED(SHGetFolderPathW(NULL, csidl, NULL, 0, dir))) {
        wchar_t full[MAX_PATH];
        wsprintfW(full, L"%s\\%s", dir, lnk_name);
        DeleteFileW(full);
    }
}

int WINAPI wWinMain(HINSTANCE hInstance, HINSTANCE hPrevInstance, LPWSTR lpCmdLine, int nCmdShow) {
    (void)hInstance;
    (void)hPrevInstance;
    (void)nCmdShow;

    BOOL silent = FALSE;
    if (lpCmdLine && (wcsstr(lpCmdLine, L"/S") || wcsstr(lpCmdLine, L"/s") || wcsstr(lpCmdLine, L"--silent"))) {
        silent = TRUE;
    }

    if (!silent) {
        int res = MessageBoxW(
            NULL,
            L"Voulez-vous désinstaller complètement 4k free video dowloader de votre ordinateur ?\n\n"
            L"(Vos vidéos déjà téléchargées dans votre dossier Téléchargements / Vidéos seront conservées.)",
            L"Désinstallation — 4k free video dowloader",
            MB_ICONQUESTION | MB_YESNO | MB_DEFBUTTON2
        );
        if (res != IDYES) {
            return 0;
        }
    }

    HWND hRunning = FindWindowW(NULL, L"4k free video dowloader");
    if (hRunning) {
        PostMessageW(hRunning, WM_CLOSE, 0, 0);
        Sleep(600);
    }

    wchar_t exe_path[MAX_PATH];
    GetModuleFileNameW(NULL, exe_path, MAX_PATH);
    wchar_t install_dir[MAX_PATH];
    lstrcpynW(install_dir, exe_path, MAX_PATH);
    wchar_t *last_slash = wcsrchr(install_dir, L'\\');
    if (last_slash) {
        *last_slash = L'\0';
    }

    const wchar_t *files[] = {
        L"4k free video dowloader.exe",
        L"WebView2Loader.dll",
        L"vidgrab-cli.exe",
        L"LICENSE"
    };
    for (int i = 0; i < 4; i++) {
        wchar_t target[MAX_PATH];
        wsprintfW(target, L"%s\\%s", install_dir, files[i]);
        SetFileAttributesW(target, FILE_ATTRIBUTE_NORMAL);
        DeleteFileW(target);
    }

    remove_shortcut_in_folder(CSIDL_COMMON_PROGRAMS, L"4k free video dowloader.lnk");
    remove_shortcut_in_folder(CSIDL_PROGRAMS, L"4k free video dowloader.lnk");
    remove_shortcut_in_folder(CSIDL_COMMON_DESKTOPDIRECTORY, L"4k free video dowloader.lnk");
    remove_shortcut_in_folder(CSIDL_DESKTOPDIRECTORY, L"4k free video dowloader.lnk");

    RegDeleteKeyW(HKEY_LOCAL_MACHINE, UNINSTALL_REG_KEY);
    RegDeleteKeyW(HKEY_CURRENT_USER, UNINSTALL_REG_KEY);

    if (!silent) {
        MessageBoxW(
            NULL,
            L"4k free video dowloader a été désinstallé avec succès.",
            L"Désinstallation terminée",
            MB_ICONINFORMATION | MB_OK
        );
    }

    wchar_t cmd_args[MAX_PATH * 3];
    wsprintfW(
        cmd_args,
        L"/c ping 127.0.0.1 -n 2 > nul & del /f /q \"%s\" & rmdir \"%s\"",
        exe_path,
        install_dir
    );
    ShellExecuteW(NULL, L"open", L"cmd.exe", cmd_args, L"C:\\", SW_HIDE);
    return 0;
}
