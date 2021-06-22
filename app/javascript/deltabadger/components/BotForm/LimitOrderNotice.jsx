import React from 'react'
import I18n from 'i18n-js'

export default () => (
  <div className="pt-3 pb-3">
    <br></br>
    <sup>*</sup> {I18n.t('bots.setup.limit_warning')}
  </div>
)
