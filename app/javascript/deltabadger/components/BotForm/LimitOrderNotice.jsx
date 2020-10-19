import React from "react";

export default () => {
  return (
    <small className="alert alert-warning db-alert--annotation">
        <sup>*</sup> The limit order bot is an experimental feature. Not all orders will get filled. To achieve good results, you need to increase the invested amount proportionally. At the moment, Deltabadger does not yet offer any help with estimating this adjustment, so your own backtesting is necessary. Use it only if you know what you're doing.
    </small>
  )
}
