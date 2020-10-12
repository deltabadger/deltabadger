import React from "react";

export default () => {
  return (
    <small className="alert alert-warning db-alert--annotation">
        The limit order bot is an experimental feature. The bot opens orders but does not track if they get filled. Our backtesting proved that historically for buy orders, ~2% gave the best results. Remember that it may take some time until those orders will get filled. Values below 0.5% should result in a relatively fast resolution, so it's an excellent place to start if you want to play with it. Limit bot DCA works the best with <i>smart intervals</i> when the number of transactions is larger.
    </small>
  )
}
