import React, { useState } from 'react'
import API from '../lib/API';

export const NewsletterForm = () => {
  const [email, setEmail] = useState("");
  const [resultInfo, setResultInfo] = useState(undefined);

  const disableSubmit = email == ''

  const handleSubmit = (evt) => {
    evt.preventDefault();
    !disableSubmit && API.addSubscriber(email).then(({data}) => {
      setEmail("")
      setResultInfo(["Success"])
    }).catch((data) => {
      setResultInfo(data.response.data.errors)
    })
  }

  return (
    <div>
      <div className="form-group">
        <input
          type="email"
          value={email}
          onChange={e => setEmail(e.target.value)}
          className="form-control"
        />
      </div>
      <div className="form-group">
        <div onClick={handleSubmit} className={`btn btn-success ${disableSubmit ? 'disabled' : ''}` }>
          Keep me in the loop!
        </div>
      </div>

      { resultInfo && resultInfo.map((text, index) => <div key={index} >{text}</div>) }
    </div>
  )
}
