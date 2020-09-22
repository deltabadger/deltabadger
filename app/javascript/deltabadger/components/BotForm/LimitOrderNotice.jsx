import React from "react";

export default () => {
  return (
    <small className="alert alert-warning db-alert--annotation">
        <sup>*</sup> The limit order bot is an experimental feature. The bot opens orders but does not track if they have been filled. Our backtesting showed that for buy orders, 2.0% below the price worked optimal in the past. <a href="#">Read more</a>
    </small>
  )
}

