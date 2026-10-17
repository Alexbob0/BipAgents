
# WorkManager (brought by the widget's Glance) creates its Room database by reflection: R8 must keep the generated classes.
-keep class * extends androidx.room.RoomDatabase { <init>(); }
-keep class androidx.work.impl.** { *; }
