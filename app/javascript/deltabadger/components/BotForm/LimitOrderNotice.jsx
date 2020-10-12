import React from "react";

export default () => {
  return (
    <small className="alert alert-warning db-alert--annotation">
        <sup>*</sup> The limit order bot is an experimental feature. The bot opens orders, but does not track if they get filled. Our backtesting proved that for buy orders historically 2% gave the best results.
    </small>
  )
}
