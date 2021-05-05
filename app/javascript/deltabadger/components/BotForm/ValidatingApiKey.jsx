import I18n from 'i18n-js'
import { Breadcrumbs } from './Breadcrumbs'
import { Progressbar } from './Progressbar'
import { Spinner } from "../Spinner";
import React from "react";


export const ValidatingApiKey = ({
}) => {

  return (
    <div className="db-bots__item db-bot db-bot--get-apikey db-bot--active">
      <div className="db-bot__header">
        <Spinner />
        <Breadcrumbs step={3} />
      </div>
      <Progressbar value={33}/>
      <div className="db-bot__form db-bot__form--apikeys">
        Your API key is being validated by us. Please wait.
      </div>
    </div>
  )
}
