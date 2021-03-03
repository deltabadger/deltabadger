import React from 'react'

export const RawHTML = ({ children, tag = 'div', ...rest }) =>
  React.createElement(tag, { dangerouslySetInnerHTML: { __html: children }, ...rest})
