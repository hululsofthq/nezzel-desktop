use objc2::DeclaredClass;
use objc2_foundation::{NSObjectProtocol, NSString};
use objc2_web_kit::{
  WKNavigation, WKNavigationAction, WKNavigationActionPolicy, WKNavigationResponse,
  WKNavigationResponsePolicy,
};

#[cfg(target_os = "ios")]
use crate::wkwebview::ios::WKWebView::WKWebView;
#[cfg(target_os = "macos")]
use objc2_web_kit::WKWebView;

use crate::PageLoadEvent;

use super::class::wry_navigation_delegate::WryNavigationDelegate;

pub(crate) fn did_commit_navigation(
  this: &WryNavigationDelegate,
  webview: &WKWebView,
  _navigation: &WKNavigation,
) {
  unsafe {
    // Call on_load_handler
    if let Some(on_page_load) = &this.ivars().on_page_load_handler {
      on_page_load(PageLoadEvent::Started);
    }

    // Inject scripts
    let mut pending_scripts = this.ivars().pending_scripts.lock().unwrap();
    if let Some(scripts) = &*pending_scripts {
      for script in scripts {
        #[cfg(feature = "tracing")]
        let handler = block2::RcBlock::new(
          |_val: *mut objc2::runtime::AnyObject, err: *mut objc2_foundation::NSError| {
            if !err.is_null() {
              let description = (*err).localizedDescription().to_string();
              tracing::debug!("Exception during script eval: {description}");
            }
          },
        );
        #[cfg(feature = "tracing")]
        let handler = Some(&*handler);
        #[cfg(not(feature = "tracing"))]
        let handler = None;

        webview.evaluateJavaScript_completionHandler(&NSString::from_str(script), handler);
      }
      *pending_scripts = None;
    }
  }
}

pub(crate) fn did_finish_navigation(
  this: &WryNavigationDelegate,
  _webview: &WKWebView,
  _navigation: &WKNavigation,
) {
  if let Some(on_page_load) = &this.ivars().on_page_load_handler {
    on_page_load(PageLoadEvent::Finished);
  }
}

// Navigation handler
pub(crate) fn navigation_policy(
  this: &WryNavigationDelegate,
  _webview: &WKWebView,
  action: &WKNavigationAction,
  handler: &block2::Block<dyn Fn(WKNavigationActionPolicy)>,
) {
  unsafe {
    // <https://developer.apple.com/documentation/webkit/wknavigationaction/shouldperformdownload>
    // Available: macOS 11.3+, iOS 14.5+
    let can_download = action.respondsToSelector(objc2::sel!(shouldPerformDownload));
    let should_download: bool = if can_download {
      action.shouldPerformDownload()
    } else {
      false
    };
    let request = action.request();
    // Nezzel patch: a request without a URL is cancelled instead of aborting.
    let Some(url) = request.URL().and_then(|url| url.absoluteString()) else {
      (*handler).call((WKNavigationActionPolicy::Cancel,));
      return;
    };

    if should_download {
      let has_download_handler = this.ivars().has_download_handler;
      // Nezzel patch: a download is a navigation first. Upstream skipped the navigation
      // handler here, so `<a download>` reached addresses the handler refuses.
      let allowed = (this.ivars().navigation_policy_function)(url.to_string());
      if has_download_handler && allowed {
        (*handler).call((WKNavigationActionPolicy::Download,));
      } else {
        (*handler).call((WKNavigationActionPolicy::Cancel,));
      }
    } else {
      let function = &this.ivars().navigation_policy_function;
      // Nezzel patch: the handler can ask whether this is the main frame
      // (`crate::navigation_main_frame`). No target frame: a new window, not this page.
      let main = action
        .targetFrame()
        .is_some_and(|frame| frame.isMainFrame());
      let new_window = action.targetFrame().is_none();
      let previous = crate::NAVIGATION_MAIN_FRAME.with(|cell| cell.replace(Some(main)));
      let previous_new = crate::NAVIGATION_NEW_WINDOW.with(|cell| cell.replace(Some(new_window)));
      // Nezzel patch: keep AppKit input with this decision, including during nested callbacks.
      #[cfg(target_os = "macos")]
      let previous_modifiers = crate::NAVIGATION_MODIFIERS.with(|cell| {
        cell.replace(Some(crate::NavigationModifiers {
          navigation_type: action.navigationType(),
          modifier_flags: action.modifierFlags(),
          button_number: action.buttonNumber(),
        }))
      });
      let allowed = function(url.to_string());
      #[cfg(target_os = "macos")]
      crate::NAVIGATION_MODIFIERS.with(|cell| cell.set(previous_modifiers));
      crate::NAVIGATION_MAIN_FRAME.with(|cell| cell.set(previous));
      crate::NAVIGATION_NEW_WINDOW.with(|cell| cell.set(previous_new));
      match allowed {
        true => (*handler).call((WKNavigationActionPolicy::Allow,)),
        false => (*handler).call((WKNavigationActionPolicy::Cancel,)),
      };
    }
  }
}

// Navigation handler
pub(crate) fn navigation_policy_response(
  this: &WryNavigationDelegate,
  _webview: &WKWebView,
  response: &WKNavigationResponse,
  handler: &block2::Block<dyn Fn(WKNavigationResponsePolicy)>,
) {
  unsafe {
    let can_show_mime_type = response.canShowMIMEType();

    if !can_show_mime_type {
      let has_download_handler = this.ivars().has_download_handler;
      if has_download_handler {
        (*handler).call((WKNavigationResponsePolicy::Download,));
        return;
      }
    }

    (*handler).call((WKNavigationResponsePolicy::Allow,));
  }
}

pub(crate) fn web_content_process_did_terminate(
  this: &WryNavigationDelegate,
  _webview: &WKWebView,
) {
  if let Some(on_web_content_process_terminate) =
    &this.ivars().on_web_content_process_terminate_handler
  {
    on_web_content_process_terminate();
  }
}
