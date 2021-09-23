import React from 'react'
import {PaginationList} from "../PaginationList";
import {CancelButton} from "./CancelButton";
import {ClosedForm} from "./ClosedForm";
export const NavigationPanel = ({
  closedFormHandler,
  handleCancel,
  step,
  showPagination,
  page,
  setPage
}) => {
  return (
    <div className="db-bots__item d-flex db-add-more-bots">
      { step === 0 && <ClosedForm handleSubmit={closedFormHandler}/> }
      { step > 0 && <CancelButton handleCancel={handleCancel} /> }
      { showPagination && <PaginationList page={page} setPage={setPage}/> }
    </div>

  )
}
