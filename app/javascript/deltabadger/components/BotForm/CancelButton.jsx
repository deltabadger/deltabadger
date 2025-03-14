import React from 'react'
export const CancelButton = ({handleCancel}) => {
  return (
    <div className="db-bots__item d-flex db-add-more-bots">
      <button onClick={() => handleCancel()} className="button button--primary button--outline">
        <i className="material-icons">close</i>
      </button>
    </div>
  )
}
