import React from 'react'
import {PaginationList} from "../PaginationList";
import {CancelButton} from "./CancelButton";
import {ClosedForm} from "./ClosedForm";
export const NavigationPanel = ({
  closedFormHandler,
  handleCancel,
  step,
  page,
  setPage,
  numberOfPages
}) => {
  return (
    <div className="db-bots__item d-flex db-add-more-bots">
      { step === 0 && <ClosedForm handleSubmit={closedFormHandler}/> }
      { step > 0 && <CancelButton handleCancel={handleCancel} /> }
      { numberOfPages > 1 && <PaginationList page={page} setPage={setPage} numberOfPages={numberOfPages} handleCancel={handleCancel}/> }
    </div>

  )
}
