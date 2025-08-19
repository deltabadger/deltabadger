# Add open tracking by extending the gem class
module AhoyEmail
  module DatabaseSubscriberExtension
    def track_open(event)
      Ahoy::Open.find_or_create_by!(campaign: event[:campaign], token: event[:token])
    end

    # extends the original method to add open tracking
    def stats(campaign)
      stats_result = super(campaign)
      opens = Ahoy::Open.where(campaign: campaign).count
      stats_result.merge(
        {
          opens: opens,
          open_rate: 100 * opens / stats_result[:sends].to_f
        }
      )
    end
  end
end

AhoyEmail::DatabaseSubscriber.prepend(AhoyEmail::DatabaseSubscriberExtension)
