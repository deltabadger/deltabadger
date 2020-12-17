import React from 'react'

export const BinanceUSInstructions = () => (
  <div className="db-exchange-instructions db-exchange-instructions--binance">
    <div className="alert alert-success mx-0" role="alert">
      <b className="alert-heading mb-2">How to get API keys from BinanceUS:</b>
      <hr/>
      <ol>
        <li>Login to your <a href="https://www.binance.us/en/home" target="_blank" rel="noopener">BinanceUS</a> account.</li>
        <li>In user menu (round icon in the top right corner) go to <b>API Management</b>.</li>
        <li>Name the new API key by pressing "Create".</li>
        <li>Confirm creation with one-time code(s).</li>
        <li>Make sure that <b>Enable Spot & Margin Trading</b> is checked.</li>
        <li>If you want to restrict access to the key only for Deltabadger, click <b>Edit restrictions</b>, check <b>Restrict access to trusted IPs only (Recommended)</b>, and add this IP in the next step: <b>3.18.70.231</b></li>
        <li>Press <b>Save</b> and confirm with another one-time code if necessary.</li>
        <li>Remember that the Secret Key is visible only during creation, so make sure you copy both keys before leaving the view.</li>
        <li>Paste both keys into the inputs above.</li>
      </ol>
    </div>
  </div>
)
