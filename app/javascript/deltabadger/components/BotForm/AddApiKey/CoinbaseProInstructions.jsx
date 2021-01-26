import React, { useState } from 'react'

export const CoinbaseProInstructions = () => (
  <div className="db-exchange-instructions db-exchange-instructions--bitbay">
    <div className="alert alert-success mx-0" role="alert">
      <b className="alert-heading mb-2">How to get API keys and Passphrase from Coinbase:</b>
      <hr/>
      <ol>
        <li>Login to your <a href="https://pro.coinbase.com/" target="_blank" rel="nofollow">Coinbase Pro</a> account.</li>
        <li>In user menu (top right corner) go to <b>API</b>.</li>
        <li>Press <b>New API Key</b>.</li>
        <li>Set permissions for:
          <ul>
            <li><b>View</b></li>
          	<li><b>Trade</b></li>
          </ul>
        </li>
        <li>Press <b>Create API Key</b>.</li>
        <li>Copy and paste your new API keys and Passphrase into the form above.</li>
      </ol>
    </div>
  </div>
)
