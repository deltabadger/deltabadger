class ClaimNftMailerPreview < ActionMailer::Preview
  def form_submission_email
    ClaimNftMailer.form_submission_email(
      'test@example.com',
      'LEGENDARY-001',
      '0x742d35Cc6634C0532925a3b844Bc454e4438f44e'
    )
  end
end
