class ClaimNftMailerPreview < ActionMailer::Preview
  def form_submission_email
    user = User.new(email: 'test@example.com', name: 'Mathias')
    subscription = Subscription.new(nft_id: 'LEGENDARY-001', eth_address: '0x742d35Cc6634C0532925a3b844Bc454e4438f44e')
    ClaimNftMailer.with(
      user: user,
      subscription: subscription
    ).form_submission_email
  end
end
