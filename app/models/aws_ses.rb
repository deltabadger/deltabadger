class AwsSes
  # https://docs.aws.amazon.com/sdk-for-ruby/v3/api/Aws/SESV2/Client.html

  REASONS_MAP = {
    'BOUNCE' => 'bounce',
    'COMPLAINT' => 'spam'
  }.freeze

  def sync_opt_outs
    result = get_opt_outs

    opted_out_user_ids = User.where(email: result.data.keys).pluck(:id)
    Caffeinate::CampaignSubscription.active.where(subscriber_id: opted_out_user_ids).find_each do |subscription|
      if result.data[subscription.subscriber.email][:time] > (subscription.resubscribed_at || subscription.created_at)
        subscription.subscriber.update!(subscribed_to_email_marketing: false)
        subscription.unsubscribe!(result.data[subscription.subscriber.email][:reason])
      end
    end
  end

  def get_opt_outs
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
    Result::Success.new(opt_outs)
  end

  private

  def client
    @client ||= ::Aws::SESV2::Client.new
  end
end
