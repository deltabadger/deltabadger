import React from 'react'
import I18n from 'i18n-js'
import CookieConsent from "react-cookie-consent";

export const CookieBanner = () => {
  return (
    <CookieConsent
      location="bottom"
      buttonText={I18n.t('cookie.agree')}
      cookieName="CookieConsentDeltabadger"
      style={{}}
      buttonStyle={{}}
      expires={150}
    >
      <span className="cookie_text">
        {I18n.t('cookie.description')}
        {" "}
        <a href="/cookies_policy" title={I18n.t('links.cookies_policy')}>{I18n.t('cookie.read_more')}</a>
      </span>
    </CookieConsent>
  )
}
