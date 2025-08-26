class AwsSes
  # https://docs.aws.amazon.com/sdk-for-ruby/v3/api/Aws/SESV2/Client.html

  REASONS_MAP = {
    'BOUNCE' => 'bounce',
    'COMPLAINT' => 'spam'
  }.freeze

  def sync_opt_outs
    opt_outs_result = opt_outs

    opted_out_user_ids = User.where(email: opt_outs_result.keys).pluck(:id)
    Caffeinate::CampaignSubscription.active.where(subscriber_id: opted_out_user_ids).find_each do |subscription|
      if opt_outs_result[subscription.subscriber.email][:time] > (subscription.resubscribed_at || subscription.created_at)
        subscription.subscriber.update!(subscribed_to_email_marketing: false)
        subscription.unsubscribe!(opt_outs_result[subscription.subscriber.email][:reason])
      end
    end
  end

  def opt_outs
    response = client.list_suppressed_destinations({
                                                     reasons: %w[BOUNCE COMPLAINT],
                                                     start_date: Time.current - (86_400 * 365),
                                                     end_date: Time.current
                                                   })

    opt_outs = {}
    response.each do |page|
      page.suppressed_destination_summaries.each do |record|
        opt_outs[record.email_address.downcase] = {
          time: record.last_update_time, # Time
          reason: REASONS_MAP[record.reason]
        }
      end
    end
    opt_outs
  end

  private

  def client
    @client ||= ::Aws::SESV2::Client.new
  end
end
