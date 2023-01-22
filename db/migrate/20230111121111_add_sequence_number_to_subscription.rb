class AddSequenceNumberToSubscription < ActiveRecord::Migration[6.0]
  def change
    add_column :subscriptions, :sequence_number, :integer

    reversible do |dir|
      dir.up do
        legendary_badger_plan = SubscriptionPlan.find_by_name(SubscriptionPlan::LEGENDARY_BADGER)
        subscriptions = Subscription.current.where(subscription_plan_id: legendary_badger_plan.id)
        subscriptions.each{|subscription| subscription.update sequence_number: subscription.send(:next_sequence_number)}
      end
    end
  end
end
