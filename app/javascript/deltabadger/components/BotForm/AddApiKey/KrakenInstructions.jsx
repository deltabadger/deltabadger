import React, { useState } from 'react'

export const KrakenInstructions = () => (
  <div className="db-exchange-instructions db-exchange-instructions--kraken">
    <div className="alert alert-success mx-0 mb-3 col" role="alert">
      <b className="alert-heading mb-2">How to get API keys from Kraken:</b>
      <hr/>
      <ol>
        <li>Login to your <a href="https://kraken.com/" target="_blank" rel="noopener">Kraken</a> account.</li>
        <li>In user menu go to <b>Settings</b> -> <b>API</b>.</li>
        <li>Press <b>Generate New Key</b>.</li>
        <li>Set permissions for:
          <ul>
          	<li><b>Query Funds</b></li>
          	<li><b>Query Open Orders & Trades</b></li>
          	<li><b>Query Closed Orders & Trades</b></li>
          	<li><b>Modify Orders</b></li>
          	<li><b>Cancel/Close Orders</b></li>
          </ul>
        </li>
        <li>Press <b>Generate Key</b>.</li>
        <li>Copy and paste your new API keys into the form above.</li>
      </ol>
    </div>
  </div>
)
