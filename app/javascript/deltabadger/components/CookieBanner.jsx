import React from 'react'
import CookieConsent from "react-cookie-consent";

export const CookieBanner = () => {
  return (
    <CookieConsent
      location="bottom"
      buttonText="I agree"
      cookieName="CookieConsentDeltabadger"
      style={{}}
      buttonStyle={{}}
      expires={150}
    >
      <span className="cookie_text">
        We use cookies to make things simpler<span className="d-none d-sm-inline"></span>.{""} <a href="/cookies_policy" title="Cookies Policy">Read more</a>
      </span>
    </CookieConsent>
  )
}
