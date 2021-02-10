import React from 'react'

export const FtxInstructions = () => (
  <div className="db-exchange-instructions db-exchange-instructions--bitbay">
    <div className="alert alert-success mx-0" role="alert">
      <b className="alert-heading mb-2">How to get API keys from FTX:</b>
      <hr/>
      <ol>
        <li>Login to your <a href="https://ftx.com" target="_blank" rel="nofollow">FTX</a> account.</li>
        <li>Go to settings menu (top right corner).</li>
        <li>Scroll down to <b>API Keys</b> section.</li>
        <li>Press <b>Create API Key</b>.</li>
        <li>Copy and paste your new API keys into the form above.</li>
      </ol>
    </div>
  </div>
)
