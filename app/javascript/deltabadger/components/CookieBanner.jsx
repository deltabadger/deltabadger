import React from 'react'
import CookieConsent from "react-cookie-consent";

export const CookieBanner = () => {
  return (
    <CookieConsent
      location="bottom"
      buttonText="OK"
      cookieName="CookieConsentDeltabadger"
      style={{}}
      buttonStyle={{}}
      expires={150}
    >
      <span className="cookie_text">
        We use cookies<span className="d-none d-sm-inline"> to enhance user experience</span>.{""} <a href="/cookies_policy" title="Cookies Policy">Read more</a>
      </span>
    </CookieConsent>
  )
}
