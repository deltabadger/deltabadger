import React from 'react'
import I18n from 'i18n-js'

export default () => (
  <small className='alert alert-warning db-alert--annotation'>
    <sup>*</sup> {I18n.t('bots.setup.limit_warning')}
  </small>
)
