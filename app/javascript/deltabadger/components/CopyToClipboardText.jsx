import React, { useState } from 'react';

const CopyToClipboardText = ({text, feedbackText}) => {
  const [isCopied, setIsCopied] = useState(false);

  const copyToClipboard = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setIsCopied(true);
    } catch (err) {
      console.error('Failed to copy text: ', err);
    }
  };

  const handleClick = () => {
    copyToClipboard();
    setTimeout(() => setIsCopied(false), 2000); // Reset after 2 seconds
  };

  return (
    <div>
      <span 
        onClick={handleClick} 
        style={{cursor: 'pointer'}} 
        className={`${isCopied ? 'text-success' : ''}`}>
        {isCopied ? feedbackText : text}
      </span>
    </div>
  );
};

export default CopyToClipboardText;