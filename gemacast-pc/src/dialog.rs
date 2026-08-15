//! Application-branded native message dialogs.

pub use rfd::{MessageButtons, MessageDialogResult, MessageLevel};

#[derive(Debug, Default)]
pub struct MessageDialog {
    title: String,
    description: String,
    level: MessageLevel,
    buttons: MessageButtons,
}

impl MessageDialog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn set_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    pub fn set_level(mut self, level: MessageLevel) -> Self {
        self.level = level;
        self
    }

    pub fn set_buttons(mut self, buttons: MessageButtons) -> Self {
        self.buttons = buttons;
        self
    }

    pub fn show(self) -> MessageDialogResult {
        #[cfg(windows)]
        {
            windows::show(self)
        }

        #[cfg(not(windows))]
        {
            rfd::MessageDialog::new()
                .set_title(self.title)
                .set_description(self.description)
                .set_level(self.level)
                .set_buttons(self.buttons)
                .show()
        }
    }
}

#[cfg(windows)]
mod windows {
    use super::{MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
    use windows_sys::Win32::{
        Foundation::{BOOL, HWND, LPARAM, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::{
                TASKDIALOG_BUTTON, TASKDIALOG_NOTIFICATIONS, TASKDIALOGCONFIG, TASKDIALOGCONFIG_0,
                TASKDIALOGCONFIG_1, TD_ERROR_ICON, TD_INFORMATION_ICON, TD_WARNING_ICON,
                TDCBF_CANCEL_BUTTON, TDCBF_NO_BUTTON, TDCBF_OK_BUTTON, TDCBF_YES_BUTTON,
                TDF_ALLOW_DIALOG_CANCELLATION, TDF_SIZE_TO_CONTENT, TDN_CREATED,
                TaskDialogIndirect,
            },
            WindowsAndMessaging::{
                ICON_BIG, ICON_SMALL, IDCANCEL, IDNO, IDOK, IDYES, LoadIconW, SendMessageW,
                WM_SETICON,
            },
        },
    };

    const APPLICATION_ICON_ID: usize = 32512;
    const ID_CUSTOM_OK: i32 = 1000;
    const ID_CUSTOM_CANCEL: i32 = 1001;
    const ID_CUSTOM_YES: i32 = 1004;
    const ID_CUSTOM_NO: i32 = 1008;

    pub fn show(dialog: MessageDialog) -> MessageDialogResult {
        let title = to_wide(&dialog.title);
        let description = to_wide(&dialog.description);
        let main_icon = match dialog.level {
            MessageLevel::Warning => TD_WARNING_ICON,
            MessageLevel::Error => TD_ERROR_ICON,
            MessageLevel::Info => TD_INFORMATION_ICON,
        };

        let (common_buttons, custom_buttons) = button_config(&dialog.buttons);
        let native_custom_buttons = custom_buttons
            .iter()
            .map(|(id, text)| TASKDIALOG_BUTTON {
                nButtonID: *id,
                pszButtonText: text.as_ptr(),
            })
            .collect::<Vec<_>>();

        let mut selected_button = 0;
        let mut selected_radio_button = 0;
        let mut verification_checked = 0;
        let config = TASKDIALOGCONFIG {
            cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
            hwndParent: std::ptr::null_mut(),
            hInstance: std::ptr::null_mut(),
            dwFlags: TDF_ALLOW_DIALOG_CANCELLATION | TDF_SIZE_TO_CONTENT,
            dwCommonButtons: common_buttons,
            pszWindowTitle: title.as_ptr(),
            Anonymous1: TASKDIALOGCONFIG_0 {
                pszMainIcon: main_icon,
            },
            pszMainInstruction: std::ptr::null(),
            pszContent: description.as_ptr(),
            cButtons: native_custom_buttons.len() as u32,
            pButtons: native_custom_buttons.as_ptr(),
            nDefaultButton: 0,
            cRadioButtons: 0,
            pRadioButtons: std::ptr::null(),
            nDefaultRadioButton: 0,
            pszVerificationText: std::ptr::null(),
            pszExpandedInformation: std::ptr::null(),
            pszExpandedControlText: std::ptr::null(),
            pszCollapsedControlText: std::ptr::null(),
            Anonymous2: TASKDIALOGCONFIG_1 {
                pszFooterIcon: std::ptr::null(),
            },
            pszFooter: std::ptr::null(),
            pfCallback: Some(set_dialog_icon),
            lpCallbackData: 0,
            cxWidth: 0,
        };

        let result = unsafe {
            TaskDialogIndirect(
                &config,
                &mut selected_button,
                &mut selected_radio_button,
                &mut verification_checked as *mut BOOL,
            )
        };

        if result != 0 {
            tracing::warn!(hresult = result, "failed to show native message dialog");
            return MessageDialogResult::Cancel;
        }

        map_result(selected_button, dialog.buttons)
    }

    fn button_config(buttons: &MessageButtons) -> (i32, Vec<(i32, Vec<u16>)>) {
        match buttons {
            MessageButtons::Ok => (TDCBF_OK_BUTTON, Vec::new()),
            MessageButtons::OkCancel => (TDCBF_OK_BUTTON | TDCBF_CANCEL_BUTTON, Vec::new()),
            MessageButtons::YesNo => (TDCBF_YES_BUTTON | TDCBF_NO_BUTTON, Vec::new()),
            MessageButtons::YesNoCancel => (
                TDCBF_YES_BUTTON | TDCBF_NO_BUTTON | TDCBF_CANCEL_BUTTON,
                Vec::new(),
            ),
            MessageButtons::OkCustom(ok) => (0, vec![(ID_CUSTOM_OK, to_wide(ok))]),
            MessageButtons::OkCancelCustom(ok, cancel) => (
                0,
                vec![
                    (ID_CUSTOM_OK, to_wide(ok)),
                    (ID_CUSTOM_CANCEL, to_wide(cancel)),
                ],
            ),
            MessageButtons::YesNoCancelCustom(yes, no, cancel) => (
                0,
                vec![
                    (ID_CUSTOM_YES, to_wide(yes)),
                    (ID_CUSTOM_NO, to_wide(no)),
                    (ID_CUSTOM_CANCEL, to_wide(cancel)),
                ],
            ),
        }
    }

    fn map_result(button: i32, buttons: MessageButtons) -> MessageDialogResult {
        match button {
            IDOK => MessageDialogResult::Ok,
            IDYES => MessageDialogResult::Yes,
            IDCANCEL => MessageDialogResult::Cancel,
            IDNO => MessageDialogResult::No,
            ID_CUSTOM_OK => match buttons {
                MessageButtons::OkCustom(label) | MessageButtons::OkCancelCustom(label, _) => {
                    MessageDialogResult::Custom(label)
                }
                _ => MessageDialogResult::Cancel,
            },
            ID_CUSTOM_CANCEL => match buttons {
                MessageButtons::OkCancelCustom(_, label)
                | MessageButtons::YesNoCancelCustom(_, _, label) => {
                    MessageDialogResult::Custom(label)
                }
                _ => MessageDialogResult::Cancel,
            },
            ID_CUSTOM_YES => match buttons {
                MessageButtons::YesNoCancelCustom(label, _, _) => {
                    MessageDialogResult::Custom(label)
                }
                _ => MessageDialogResult::Cancel,
            },
            ID_CUSTOM_NO => match buttons {
                MessageButtons::YesNoCancelCustom(_, label, _) => {
                    MessageDialogResult::Custom(label)
                }
                _ => MessageDialogResult::Cancel,
            },
            _ => MessageDialogResult::Cancel,
        }
    }

    fn to_wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe extern "system" fn set_dialog_icon(
        hwnd: HWND,
        notification: TASKDIALOG_NOTIFICATIONS,
        _wparam: WPARAM,
        _lparam: LPARAM,
        _callback_data: isize,
    ) -> i32 {
        if notification == TDN_CREATED {
            let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
            if !instance.is_null() {
                let icon = unsafe { LoadIconW(instance, APPLICATION_ICON_ID as *const u16) };
                if !icon.is_null() {
                    unsafe {
                        SendMessageW(hwnd, WM_SETICON, ICON_BIG as WPARAM, icon as LPARAM);
                        SendMessageW(hwnd, WM_SETICON, ICON_SMALL as WPARAM, icon as LPARAM);
                    }
                }
            }
        }

        0
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn wide_strings_are_null_terminated() {
            assert_eq!(to_wide("Gemacast"), [71, 101, 109, 97, 99, 97, 115, 116, 0]);
        }

        #[test]
        fn maps_custom_button_results_to_their_labels() {
            assert_eq!(
                map_result(
                    ID_CUSTOM_NO,
                    MessageButtons::YesNoCancelCustom(
                        "Allow".into(),
                        "Reject".into(),
                        "Later".into(),
                    ),
                ),
                MessageDialogResult::Custom("Reject".into())
            );
        }
    }
}
