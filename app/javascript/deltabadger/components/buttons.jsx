import React, { useState } from 'react';

export const StartButton = ({onClick}) => (
  <div onClick={onClick} className="btn btn-success"><span>Start</span> <i className="material-icons">play_arrow</i></div>
)
export const StopButton = ({onClick}) => (
  <div onClick={onClick} className="btn btn-outline-primary"><span>Pause</span> <i className="material-icons">pause</i></div>
)

// export const RemoveButton = ({onClick}) => (
//   <div
//     onClick={onClick}
//     className="btn btn-link btn--reset"
//   >
//     <i className="material-icons">sync</i>
//     <span>Reset</span>
//   </div>
// )

export const RemoveButton = ({onClick}) => {
  const [isOpen, setOpen] = useState(false)

  return(
    <div>
    <div
      onClick={() => setOpen(true) }
      className="btn btn-link btn--reset"
    >
      <i className="material-icons">sync</i>
      <span>Reset</span>
    </div>

    { isOpen &&
      <div> MODAL
      <div onClick={() => {onClick() && setOpen(false)}} className="btn btn-primary">Remove!!!!!</div>
      <div onClick={() => {setOpen(false)}} className="btn btn-primary">CloseModal</div>
      </div>
    }
    </div>
  )
}

// export const RemoveButton = ({onClick}) => (
//   <div>
//     <div className="btn btn-link btn--reset" data-toggle="modal" data-target="#exampleModal" >
//       <i className="material-icons">sync</i>
//       <span>Reset</span>
//     </div>

//     <div className="modal fade" id="exampleModal" tabindex="-1" role="dialog" aria-labelledby="exampleModalLabel" aria-hidden="true">
//       <div className="modal-dialog" role="document">
//         <div className="modal-content">
//           <div className="modal-header">
//             <h5 className="modal-title" id="exampleModalLabel">Modal title</h5>
//             <button type="button" className="close" data-dismiss="modal" aria-label="Close">
//               <span aria-hidden="true">&times;</span>
//             </button>
//           </div>
//           <div className="modal-body">
//             ...
//           </div>
//           <div className="modal-footer">
//             <button type="button" className="btn btn-secondary" data-dismiss="modal">Close</button>
//             <button onClick={onClick} type="button" className="btn btn-primary" data-dismiss="modal">Save changes</button>
//           </div>
//         </div>
//       </div>
//     </div>
//   </div>
// )

export const CloseButton = ({onClick}) => (
  <div
    onClick={onClick}
    className="btn btn-link btn--reset"
  >
    <i className="material-icons">close</i>
    <span>Close</span>
  </div>
)

export const ExchangeButton = ({ handleClick, exchange }) => (
  <div className={`col-sm-6 col-md-4 db-bot__exchanges__item db-bot__exchanges__item--${exchange.name.toLowerCase()}`} onClick={ () => handleClick(exchange.id) }></div>
)
